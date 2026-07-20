use std::fmt;

use crate::error::{IdentityError, NythError, OverlayError};
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

#[derive(Debug)]
pub enum UnmountArgsError {
    MissingFlagValue { flag: &'static str },
    MissingForUser,
    UnexpectedArgument { raw: String },
}

impl fmt::Display for UnmountArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFlagValue { flag } => write!(f, "{flag} requires a value"),
            Self::MissingForUser => write!(f, "--for-user <name> is required"),
            Self::UnexpectedArgument { raw } => write!(
                f,
                "unexpected argument '{raw}', expected --for-user or --purge"
            ),
        }
    }
}

impl std::error::Error for UnmountArgsError {}

/// Parses `nyth unmount --for-user <name> [--purge]`
pub fn parse_unmount_args(args: &[String]) -> Result<UnmountArgs, UnmountArgsError> {
    let mut for_user = None;
    let mut purge = false;
    let mut remaining = args.iter();

    while let Some(arg) = remaining.next() {
        match arg.as_str() {
            "--for-user" => {
                let raw = remaining
                    .next()
                    .ok_or(UnmountArgsError::MissingFlagValue { flag: "--for-user" })?;
                for_user = Some(raw.clone());
            }
            "--purge" => purge = true,
            other => {
                return Err(UnmountArgsError::UnexpectedArgument {
                    raw: other.to_string(),
                });
            }
        }
    }

    Ok(UnmountArgs {
        for_user: for_user.ok_or(UnmountArgsError::MissingForUser)?,
        purge,
    })
}

/// `nyth unmount`: unmounts the overlay and the read-only home snapshot for the target user.
/// `upper`/`work` are left in place unless `--purge` is given
pub fn run_unmount(args: &UnmountArgs) -> Result<(), NythError> {
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

    let state = current_overlay_state(&identity.dir).map_err(NythError::Overlay)?;
    if state == OverlayState::NotMounted {
        return Err(NythError::Overlay(OverlayError::NotMounted {
            user: args.for_user.clone(),
        }));
    }

    let paths = NythPaths::for_user(&args.for_user);

    unmount_overlay_and_snapshot(&identity.dir, &paths).map_err(NythError::Overlay)?;

    if args.purge {
        unmount_persistent_tmpfs(&paths).map_err(NythError::Overlay)?;
    }

    Ok(())
}
