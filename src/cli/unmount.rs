use eros::{Context, bail, ensure};

use crate::sys::overlay::{
    OverlayState, current_overlay_state, unmount_overlay_and_snapshot, unmount_persistent_tmpfs,
};
use crate::sys::paths::NythPaths;

/// argv-parsed inputs to `nyth unmount`
#[derive(Debug, Clone, Default)]
pub struct UnmountArgs {
    pub for_user: String,
    pub purge: bool,
}

/// Parses `nyth unmount --for-user <name> [--purge]`
pub fn parse_unmount_args(args: &[String]) -> eros::Result<UnmountArgs> {
    let mut for_user = None;
    let mut purge = false;
    let mut remaining = args.iter();

    while let Some(arg) = remaining.next() {
        match arg.as_str() {
            "--for-user" => {
                let Some(raw) = remaining.next() else {
                    bail!("--for-user requires a value");
                };
                for_user = Some(raw.clone());
            }
            "--purge" => purge = true,
            other => bail!(
                "unexpected argument '{}', expected --for-user or --purge",
                other
            ),
        }
    }

    let Some(for_user) = for_user else {
        bail!("--for-user <name> is required");
    };

    Ok(UnmountArgs { for_user, purge })
}

/// `nyth unmount`: unmounts overlay and home snapshot. Keeps `upper`/`work` unless `--purge`.
pub fn run_unmount(args: &UnmountArgs) -> eros::Result<()> {
    ensure!(
        nix::unistd::geteuid().is_root(),
        "nyth must run as root: mount/unmount act on another user's $HOME and need CAP_SYS_ADMIN on the host, there is no user namespace to fall back to"
    );

    // `Err` = lookup failed, `Ok(None)` = user doesn't exist
    let identity = nix::unistd::User::from_name(&args.for_user)
        .with_context(|| format!("looking up the passwd entry for '{}'", args.for_user))?
        .ok_or_else(|| eros::error!("no passwd entry found for user '{}'", args.for_user))?;

    if current_overlay_state(&identity.dir)? == OverlayState::NotMounted {
        bail!("nyth is not mounted for user '{}'", args.for_user);
    }

    let paths = NythPaths::for_user(&args.for_user);

    unmount_overlay_and_snapshot(&identity.dir, &paths)
        .with_user_context(|| format!("unmounting the overlay over {}", identity.dir.display()))?;

    if args.purge {
        unmount_persistent_tmpfs(&paths)
            .with_user_context(|| format!("purging {}", paths.root.display()))?;
    }

    Ok(())
}
