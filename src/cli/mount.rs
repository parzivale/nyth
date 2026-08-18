use std::path::PathBuf;

use eros::{Context, bail, ensure};

use crate::sys::overlay::{
    OverlayState, current_overlay_state, materialize_home_files, mount_home_snapshot,
    mount_overlay, provision_persistent_tmpfs, set_ownership,
};
use crate::sys::paths::NythPaths;

/// argv-parsed inputs to `nyth mount`
#[derive(Debug, Clone, Default)]
pub struct MountArgs {
    pub for_user: String,
    pub home_files: PathBuf,
}

/// Parses `nyth mount --for-user <name> --home-files <path>`
pub fn parse_mount_args(args: &[String]) -> eros::Result<MountArgs> {
    let mut for_user = None;
    let mut home_files = None;
    let mut remaining = args.iter();

    while let Some(arg) = remaining.next() {
        match arg.as_str() {
            "--for-user" => {
                let Some(raw) = remaining.next() else {
                    bail!("--for-user requires a value");
                };
                for_user = Some(raw.clone());
            }
            "--home-files" => {
                let Some(raw) = remaining.next() else {
                    bail!("--home-files requires a value");
                };
                home_files = Some(PathBuf::from(raw));
            }
            other => bail!(
                "unexpected argument '{}', expected --for-user or --home-files",
                other
            ),
        }
    }

    let Some(for_user) = for_user else {
        bail!("--for-user <name> is required");
    };
    let Some(home_files) = home_files else {
        bail!("--home-files <path> is required");
    };

    Ok(MountArgs {
        for_user,
        home_files,
    })
}

/// `nyth mount`: provisions `/run/nyth/<name>/`, snapshots $HOME, materializes home-files, mounts overlay.
pub fn run_mount(args: &MountArgs) -> eros::Result<()> {
    ensure!(
        nix::unistd::geteuid().is_root(),
        "nyth must run as root: mount/unmount act on another user's $HOME and need CAP_SYS_ADMIN on the host, there is no user namespace to fall back to"
    );

    // `Err` = lookup failed, `Ok(None)` = user doesn't exist
    let identity = nix::unistd::User::from_name(&args.for_user)
        .with_context(|| format!("looking up the passwd entry for '{}'", args.for_user))?
        .ok_or_else(|| eros::error!("no passwd entry found for user '{}'", args.for_user))?;

    if current_overlay_state(&identity.dir)? == OverlayState::Mounted {
        bail!("nyth is already mounted for user '{}'", args.for_user);
    }

    let paths = NythPaths::for_user(&args.for_user);

    // One user-facing summary per step; per-syscall detail stays in `#[context]`
    provision_persistent_tmpfs(&paths, identity.uid, identity.gid)
        .with_user_context(|| format!("provisioning {}", paths.root.display()))?;
    mount_home_snapshot(&identity.dir, &paths)
        .with_user_context(|| format!("snapshotting {}", identity.dir.display()))?;
    materialize_home_files(&paths, &args.home_files, identity.uid, identity.gid)
        .with_user_context(|| format!("materializing home-files from {}", args.home_files.display()))?;

    // upper/work are created by root, but must be writable by the target user
    set_ownership(&paths.upper, identity.uid, identity.gid)
        .with_user_context(|| format!("handing upper/work to '{}'", args.for_user))?;
    set_ownership(&paths.work, identity.uid, identity.gid)
        .with_user_context(|| format!("handing upper/work to '{}'", args.for_user))?;

    mount_overlay(&paths, &identity.dir)
        .with_user_context(|| format!("mounting the overlay over {}", identity.dir.display()))?;
    Ok(())
}
