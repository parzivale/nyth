use std::ffi::{CStr, OsString};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use nix::unistd::{Gid, Uid};
use rustix::fs::CWD;
use rustix::mount::{
    FsMountFlags, FsOpenFlags, MountAttrFlags, MountFlags, MoveMountFlags, UnmountFlags,
    fsconfig_create, fsconfig_set_string, fsmount, fsopen, mount, mount_remount, move_mount,
    unmount,
};

use crate::error::OverlayError;
use crate::sys::paths::NythPaths;

/// Whether the overlay is currently mounted over a given target `$HOME`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayState {
    Mounted,
    NotMounted,
}

/// Scans `/proc/self/mountinfo` for a mount point exactly at `home`
pub fn current_overlay_state(home: &Path) -> Result<OverlayState, OverlayError> {
    let file =
        fs::File::open("/proc/self/mountinfo").map_err(|e| OverlayError::StateCheckFailed {
            message: e.to_string(),
        })?;

    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| OverlayError::StateCheckFailed {
            message: e.to_string(),
        })?;
        // mountinfo(5): "... mount_id parent_id major:minor root mount_point ..."
        if let Some(mount_point) = line.split_whitespace().nth(4)
            && Path::new(mount_point) == home
        {
            return Ok(OverlayState::Mounted);
        }
    }
    Ok(OverlayState::NotMounted)
}

/// Sets up `/run/nyth/<name>/` as a persistent tmpfs, owned end-to-end by the target user, with the 4 subdirectories underneath it
pub fn provision_persistent_tmpfs(
    paths: &NythPaths,
    uid: Uid,
    gid: Gid,
) -> Result<(), OverlayError> {
    create_root_dir(&paths.root)?;
    mount_tmpfs(&paths.root)?;
    set_ownership(&paths.root, uid, gid)?;

    for dir in [
        &paths.lower,
        &paths.home_snapshot,
        &paths.upper,
        &paths.work,
    ] {
        create_dir_idempotent(dir)?;
    }
    Ok(())
}

fn create_root_dir(root: &Path) -> Result<(), OverlayError> {
    // `/run/nyth` doesn't necessarily exist yet - recursive() so this doesn't fail with ENOENT the first time it runs on a given machin
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
    {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(OverlayError::PersistentTmpfsFailed {
            errno: nix::errno::Errno::from_raw(e.raw_os_error().unwrap_or(0)),
        }),
    }
}

fn create_dir_idempotent(path: &Path) -> Result<(), OverlayError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(OverlayError::PersistentTmpfsFailed {
            errno: nix::errno::Errno::from_raw(e.raw_os_error().unwrap_or(0)),
        }),
    }
}

fn mount_tmpfs(path: &Path) -> Result<(), OverlayError> {
    mount(
        "tmpfs",
        path,
        "tmpfs",
        MountFlags::NOSUID | MountFlags::NODEV,
        None::<&CStr>,
    )
    .map_err(|e| OverlayError::PersistentTmpfsFailed {
        errno: nix::errno::Errno::from_raw(e.raw_os_error()),
    })
}

/// Read-only bind mount of the target's $HOME as it was before the overlay goes on top
pub fn mount_home_snapshot(home: &Path, paths: &NythPaths) -> Result<(), OverlayError> {
    bind_mount_readonly(home, &paths.home_snapshot).map_err(|e| OverlayError::HomeSnapshotFailed {
        errno: nix::errno::Errno::from_raw(e.raw_os_error()),
    })
}

/// Bind mount + remount read-only in one call, raw errno on failure
fn bind_mount_readonly(source: &Path, target: &Path) -> Result<(), rustix::io::Errno> {
    bind_mount(source, target)?;
    remount_readonly(target)
}

fn bind_mount(source: &Path, target: &Path) -> Result<(), rustix::io::Errno> {
    mount(
        source,
        target,
        "",
        MountFlags::BIND | MountFlags::NOSUID | MountFlags::NODEV,
        None::<&CStr>,
    )
}

// Two-step bind+remount (MS_RDONLY ignored on initial MS_BIND).
// Flags repeated on both calls: if source's host mount already has them locked (e.g. /tmp nosuid,nodev), omitting them here gets EPERM (mount_namespaces(7))
fn remount_readonly(target: &Path) -> Result<(), rustix::io::Errno> {
    // mount_remount adds MS_REMOUNT itself, leaving MS_BIND | MS_NOSUID | MS_NODEV here
    mount_remount(
        target,
        MountFlags::BIND | MountFlags::NOSUID | MountFlags::NODEV,
        "",
    )
}

/// Copies Home Manager's fully-merged `home-files` derivation (the same `$out` HM itself would symlink into `$HOME`
pub fn materialize_home_files(
    paths: &NythPaths,
    home_files: &Path,
    uid: Uid,
    gid: Gid,
) -> Result<(), OverlayError> {
    copy_tree_dereferenced(home_files, &paths.lower).map_err(|e| {
        OverlayError::HomeFilesMaterializeFailed {
            path: home_files.to_path_buf(),
            errno: nix::errno::Errno::from_raw(e.raw_os_error().unwrap_or(0)),
        }
    })?;

    chown_tree(&paths.lower, uid, gid)
}

/// Recursively copies `source` into `destination`, following symlinks
fn copy_tree_dereferenced(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = fs::metadata(source)?; // follows symlinks, unlike symlink_metadata

    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree_dereferenced(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination).map(|_| ())
    }
}

/// `chown`s `path` and, if it's a directory, everything underneath it.
fn chown_tree(path: &Path, uid: Uid, gid: Gid) -> Result<(), OverlayError> {
    set_ownership(path, uid, gid)?;

    if path.is_dir() {
        let entries = fs::read_dir(path).map_err(|e| OverlayError::OwnershipFailed {
            path: path.to_path_buf(),
            errno: nix::errno::Errno::from_raw(e.raw_os_error().unwrap_or(0)),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| OverlayError::OwnershipFailed {
                path: path.to_path_buf(),
                errno: nix::errno::Errno::from_raw(e.raw_os_error().unwrap_or(0)),
            })?;
            chown_tree(&entry.path(), uid, gid)?;
        }
    }
    Ok(())
}

pub fn mount_overlay(paths: &NythPaths, target: &Path) -> Result<(), OverlayError> {
    let fs_fd = open_overlay_fs()?;

    set_lowerdir(&fs_fd, &paths.lower, &paths.home_snapshot)?;
    set_dir_option(&fs_fd, "upperdir", &paths.upper)?;
    set_dir_option(&fs_fd, "workdir", &paths.work)?;
    fsconfig_create(&fs_fd).map_err(|e| mount_failed(target, e))?;

    let mount_fd = fsmount(
        &fs_fd,
        FsMountFlags::FSMOUNT_CLOEXEC,
        MountAttrFlags::empty(),
    )
    .map_err(|e| mount_failed(target, e))?;

    move_mount(
        &mount_fd,
        "",
        CWD,
        target,
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
    )
    .map_err(|e| mount_failed(target, e))
}

/// ENOSYS = no new mount API (kernel < 5.2); EOPNOTSUPP/ENODEV = overlay module not loaded
fn open_overlay_fs() -> Result<OwnedFd, OverlayError> {
    fsopen("overlay", FsOpenFlags::FSOPEN_CLOEXEC).map_err(|e| match e {
        rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NODEV => {
            OverlayError::OverlayApiUnsupported {
                errno: nix::errno::Errno::from_raw(e.raw_os_error()),
            }
        }
        other => mount_failed(Path::new("overlay"), other),
    })
}

fn set_lowerdir(fs_fd: &OwnedFd, lower: &Path, home_snapshot: &Path) -> Result<(), OverlayError> {
    let mut value = lower.as_os_str().as_bytes().to_vec();
    value.push(b':');
    value.extend_from_slice(home_snapshot.as_os_str().as_bytes());
    let value = OsString::from_vec(value);

    fsconfig_set_string(fs_fd, "lowerdir", value.as_os_str()).map_err(|e| mount_failed(lower, e))
}

fn set_dir_option(fs_fd: &OwnedFd, key: &str, dir: &Path) -> Result<(), OverlayError> {
    fsconfig_set_string(fs_fd, key, dir.as_os_str()).map_err(|e| mount_failed(dir, e))
}

fn mount_failed(target: &Path, e: rustix::io::Errno) -> OverlayError {
    OverlayError::MountFailed {
        target: target.to_path_buf(),
        errno: nix::errno::Errno::from_raw(e.raw_os_error()),
    }
}

/// Unmounts the overlay at `target` and the read-only home snapshot underneath it
pub fn unmount_overlay_and_snapshot(target: &Path, paths: &NythPaths) -> Result<(), OverlayError> {
    unmount_one(target)?;
    unmount_one(&paths.home_snapshot)
}

/// Additionally tears down the persistent tmpfs itself (`nyth unmount --purge`): `upper`/`work` go with it
pub fn unmount_persistent_tmpfs(paths: &NythPaths) -> Result<(), OverlayError> {
    unmount_one(&paths.root)
}

fn unmount_one(target: &Path) -> Result<(), OverlayError> {
    unmount(target, UnmountFlags::empty()).map_err(|e| OverlayError::UnmountFailed {
        target: target.to_path_buf(),
        errno: nix::errno::Errno::from_raw(e.raw_os_error()),
    })
}

/// `chown`s `path` to `uid`/`gid`. `upper`/`work` are created by root but need to be writable by the target user's own processes running inside the overlay
pub fn set_ownership(
    path: &Path,
    uid: nix::unistd::Uid,
    gid: nix::unistd::Gid,
) -> Result<(), OverlayError> {
    let owner = rustix::fs::Uid::from_raw(uid.as_raw());
    let group = rustix::fs::Gid::from_raw(gid.as_raw());
    rustix::fs::chown(path, Some(owner), Some(group)).map_err(|e| OverlayError::OwnershipFailed {
        path: path.to_path_buf(),
        errno: nix::errno::Errno::from_raw(e.raw_os_error()),
    })
}
