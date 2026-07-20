use std::fmt;
use std::path::PathBuf;

use crate::error::{IdentityError, NythError, OverlayError};
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

#[derive(Debug)]
pub enum MountArgsError {
    MissingFlagValue { flag: &'static str },
    MissingForUser,
    MissingHomeFiles,
    UnexpectedArgument { raw: String },
}

impl fmt::Display for MountArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFlagValue { flag } => write!(f, "{flag} requires a value"),
            Self::MissingForUser => write!(f, "--for-user <name> is required"),
            Self::MissingHomeFiles => write!(f, "--home-files <path> is required"),
            Self::UnexpectedArgument { raw } => write!(
                f,
                "unexpected argument '{raw}', expected --for-user or --home-files"
            ),
        }
    }
}

impl std::error::Error for MountArgsError {}

/// Parses `nyth mount --for-user <name> [--watched-path <rel>]...`
pub fn parse_mount_args(args: &[String]) -> Result<MountArgs, MountArgsError> {
    let mut for_user = None;
    let mut home_files = None;
    let mut remaining = args.iter();

    while let Some(arg) = remaining.next() {
        match arg.as_str() {
            "--for-user" => {
                let raw = remaining
                    .next()
                    .ok_or(MountArgsError::MissingFlagValue { flag: "--for-user" })?;
                for_user = Some(raw.clone());
            }
            "--home-files" => {
                let raw = remaining.next().ok_or(MountArgsError::MissingFlagValue {
                    flag: "--home-files",
                })?;
                home_files = Some(PathBuf::from(raw));
            }
            other => {
                return Err(MountArgsError::UnexpectedArgument {
                    raw: other.to_string(),
                });
            }
        }
    }

    Ok(MountArgs {
        for_user: for_user.ok_or(MountArgsError::MissingForUser)?,
        home_files: home_files.ok_or(MountArgsError::MissingHomeFiles)?,
    })
}

/// `nyth mount`: checks `geteuid() == 0`, resolves the target's identity, provisions `/run/nyth/<name>/`, snapshots the target's real $HOME read-only, resolves watched-paths into `lower/`, and mounts the overlay over the target's $HOME
pub fn run_mount(args: &MountArgs) -> Result<(), NythError> {
    nix::unistd::geteuid()
        .is_root()
        .then_some(())
        .ok_or(NythError::Identity(IdentityError::NotRunningAsRoot))?;

    let identity = nix::unistd::User::from_name(&args.for_user)
        .map_err(|err| {
            NythError::Identity(IdentityError::HomeLookupFailed {
                name: args.for_user.to_owned(),
                errno: err,
            })
        })?
        .ok_or(NythError::Identity(IdentityError::UserNotFound {
            name: args.for_user.to_owned(),
        }))?;

    let already_mounted = current_overlay_state(&identity.dir).map_err(NythError::Overlay)?;
    if already_mounted == OverlayState::Mounted {
        return Err(NythError::Overlay(OverlayError::AlreadyMounted {
            user: args.for_user.clone(),
        }));
    }

    let paths = NythPaths::for_user(&args.for_user);

    provision_persistent_tmpfs(&paths, identity.uid, identity.gid).map_err(NythError::Overlay)?;
    mount_home_snapshot(&identity.dir, &paths).map_err(NythError::Overlay)?;
    materialize_home_files(&paths, &args.home_files, identity.uid, identity.gid)
        .map_err(NythError::Overlay)?;

    // upper/work are created by root; the target user's own processes running inside the overlay need to be able to write to them
    set_ownership(&paths.upper, identity.uid, identity.gid).map_err(NythError::Overlay)?;
    set_ownership(&paths.work, identity.uid, identity.gid).map_err(NythError::Overlay)?;

    mount_overlay(&paths, &identity.dir).map_err(NythError::Overlay)
}
