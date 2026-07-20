use std::fmt;
use std::path::PathBuf;

use nix::errno::Errno;

#[derive(Debug)]
pub enum IdentityError {
    /// `geteuid() != 0` - checked first, before any mount attempt
    NotRunningAsRoot,
    /// `getpwnam_r` for `--for-user <name>` found no entry.
    UserNotFound { name: String },
    HomeLookupFailed {
        name: String,
        errno: nix::errno::Errno,
    },
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunningAsRoot => write!(
                f,
                "nyth must run as root: mount/unmount act on another user's $HOME and need CAP_SYS_ADMIN on the host, there is no user namespace to fall back to"
            ),
            Self::UserNotFound { name } => {
                write!(f, "no passwd entry found for user '{name}'")
            }
            Self::HomeLookupFailed { name, errno } => write!(
                f,
                "failed to look up passwd entry for '{name}' (errno {errno})"
            ),
        }
    }
}

impl std::error::Error for IdentityError {}

#[derive(Debug)]
pub enum OverlayError {
    AlreadyMounted { user: String },
    NotMounted { user: String },
    PersistentTmpfsFailed { errno: Errno },
    HomeSnapshotFailed { errno: Errno },
    HomeFilesMaterializeFailed { path: PathBuf, errno: Errno },
    OverlayApiUnsupported { errno: Errno },
    MountFailed { target: PathBuf, errno: Errno },
    UnmountFailed { target: PathBuf, errno: Errno },
    OwnershipFailed { path: PathBuf, errno: Errno },
    StateCheckFailed { message: String },
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyMounted { user } => {
                write!(f, "nyth is already mounted for user '{user}'")
            }
            Self::NotMounted { user } => {
                write!(f, "nyth is not mounted for user '{user}'")
            }
            Self::PersistentTmpfsFailed { errno } => write!(
                f,
                "failed to set up /run/nyth/<user> persistent tmpfs (errno {errno})"
            ),
            Self::HomeSnapshotFailed { errno } => write!(
                f,
                "failed to create read-only home snapshot (errno {errno})"
            ),
            Self::HomeFilesMaterializeFailed { path, errno } => write!(
                f,
                "failed to materialize home-files tree from {} into lower/ (errno {errno})",
                path.display()
            ),
            Self::OverlayApiUnsupported { errno } => write!(
                f,
                "kernel too old or overlay filesystem module not loaded (errno {errno})"
            ),
            Self::MountFailed { target, errno } => {
                write!(
                    f,
                    "failed to mount overlay at {} (errno {errno})",
                    target.display()
                )
            }
            Self::UnmountFailed { target, errno } => {
                write!(f, "failed to unmount {} (errno {errno})", target.display())
            }
            Self::OwnershipFailed { path, errno } => write!(
                f,
                "failed to set ownership of {} (errno {errno})",
                path.display()
            ),
            Self::StateCheckFailed { message } => {
                write!(f, "failed to check overlay state: {message}")
            }
        }
    }
}

impl std::error::Error for OverlayError {}

/// Why a `PendingChange` at `path` was refused, distinct from an I/O failure while applying one that is committable
#[derive(Debug)]
pub enum NotCommittableReason {
    /// Rendered by a `programs.*` module from Nix options; no source file in the repo to write to
    Generated,
    /// Not managed by Home Manager at all
    Untracked,
}

#[derive(Debug)]
pub enum NythError {
    Identity(IdentityError),
    Overlay(OverlayError),
    Status(StatusError),
    WatchedPathEscapesHome {
        path: PathBuf,
    },
    MountIoFailed {
        path: PathBuf,
        message: String,
    },
    CommitIoFailed {
        path: PathBuf,
        message: String,
    },
    NotCommittable {
        path: PathBuf,
        reason: NotCommittableReason,
    },
}

impl fmt::Display for NythError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity(e) => write!(f, "{e}"),
            Self::Overlay(e) => write!(f, "{e}"),
            Self::Status(e) => write!(f, "{e}"),
            Self::WatchedPathEscapesHome { path } => {
                write!(f, "watched path {} escapes $HOME", path.display())
            }
            Self::MountIoFailed { path, message } => {
                write!(f, "mount setup failed at {}: {message}", path.display())
            }
            Self::CommitIoFailed { path, message } => {
                write!(f, "commit failed at {}: {message}", path.display())
            }
            Self::NotCommittable {
                path,
                reason: NotCommittableReason::Generated,
            } => write!(
                f,
                "{} is rendered by a programs.* module, not backed by a repo file — nothing to commit it to",
                path.display()
            ),
            Self::NotCommittable {
                path,
                reason: NotCommittableReason::Untracked,
            } => write!(
                f,
                "{} is not managed by Home Manager, cannot commit an untracked path",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NythError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Identity(e) => Some(e),
            Self::Overlay(e) => Some(e),
            Self::Status(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum StatusError {
    ScanFailed { path: PathBuf, message: String },
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScanFailed { path, message } => {
                write!(f, "failed to scan {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for StatusError {}
