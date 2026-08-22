use std::ffi::{CStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::io::{BufRead, BufReader};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use eros::{Context, ErrorUnion, IntoUnion, ReshapeUnion, context};
use nix::unistd::{Gid, Uid};
use rustix::fs::CWD;
use rustix::io::Errno;
use rustix::mount::{
    FsMountFlags, FsOpenFlags, MountAttrFlags, MountFlags, MoveMountFlags, UnmountFlags,
    fsconfig_create, fsconfig_set_string, fsmount, fsopen, mount, mount_remount, move_mount,
    unmount,
};

use crate::sys::paths::NythPaths;

/// Whether the overlay is currently mounted over a given target `$HOME`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayState {
    Mounted,
    NotMounted,
}

/// Kernel can't give us an overlay filesystem at all (vs. this particular mount failing).
#[derive(Debug)]
pub struct OverlayUnsupported {
    pub errno: Errno,
}

impl fmt::Display for OverlayUnsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "kernel too old or overlay filesystem module not loaded (errno {}): load the `overlay` module, or run a kernel with the new mount API (>= 5.2)",
            self.errno
        )
    }
}

impl std::error::Error for OverlayUnsupported {}

/// Scans `/proc/self/mountinfo` for a mount point exactly at `home`
#[context("checking whether nyth is already mounted at {}", home.display())]
pub fn current_overlay_state(home: &Path) -> eros::Result<OverlayState, (io::Error,)> {
    let file = fs::File::open("/proc/self/mountinfo")?;

    for line in BufReader::new(file).lines() {
        let line = line?;
        // mountinfo(5): "... mount_id parent_id major:minor root mount_point ..."
        if let Some(mount_point) = line.split_whitespace().nth(4)
            && Path::new(mount_point) == home
        {
            return Ok(OverlayState::Mounted);
        }
    }
    Ok(OverlayState::NotMounted)
}

/// Sets up `/run/nyth/<name>/` as a persistent tmpfs owned by the target user, with its 4 subdirs
pub fn provision_persistent_tmpfs(
    paths: &NythPaths,
    uid: Uid,
    gid: Gid,
) -> eros::Result<(), (io::Error, Errno)> {
    create_root_dir(&paths.root).widen()?;
    mount_tmpfs(&paths.root).widen()?;
    set_ownership(&paths.root, uid, gid).widen()?;

    for dir in [
        &paths.lower,
        &paths.home_snapshot,
        &paths.upper,
        &paths.work,
    ] {
        create_dir_idempotent(dir).widen()?;
    }
    Ok(())
}

#[context("creating {}", root.display())]
fn create_root_dir(root: &Path) -> eros::Result<(), (io::Error,)> {
    // `/run/nyth` may not exist yet; recursive() avoids ENOENT on first run
    if let Err(e) = fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        && e.kind() != io::ErrorKind::AlreadyExists
    {
        Err(e)?;
    }
    Ok(())
}

#[context("creating {}", path.display())]
fn create_dir_idempotent(path: &Path) -> eros::Result<(), (io::Error,)> {
    if let Err(e) = fs::create_dir(path)
        && e.kind() != io::ErrorKind::AlreadyExists
    {
        Err(e)?;
    }
    Ok(())
}

#[context("mounting tmpfs at {}", path.display())]
fn mount_tmpfs(path: &Path) -> eros::Result<(), (Errno,)> {
    mount(
        "tmpfs",
        path,
        "tmpfs",
        MountFlags::NOSUID | MountFlags::NODEV,
        None::<&CStr>,
    )?;
    Ok(())
}

/// Read-only bind mount of the target's $HOME as it was before the overlay goes on top
#[context("snapshotting {} read-only at {}", home.display(), paths.home_snapshot.display())]
pub fn mount_home_snapshot(home: &Path, paths: &NythPaths) -> eros::Result<(), (Errno,)> {
    bind_mount(home, &paths.home_snapshot)?;
    remount_readonly(&paths.home_snapshot)?;
    Ok(())
}

fn bind_mount(source: &Path, target: &Path) -> Result<(), Errno> {
    mount(
        source,
        target,
        "",
        MountFlags::BIND | MountFlags::NOSUID | MountFlags::NODEV,
        None::<&CStr>,
    )
}

// Two-step bind+remount (MS_RDONLY ignored on initial MS_BIND).
// Flags repeated on both calls, or a locked host mount (e.g. /tmp nosuid) gets EPERM.
fn remount_readonly(target: &Path) -> Result<(), Errno> {
    // mount_remount adds MS_REMOUNT itself, leaving MS_BIND | MS_RDONLY | MS_NOSUID | MS_NODEV here
    mount_remount(
        target,
        MountFlags::BIND | MountFlags::RDONLY | MountFlags::NOSUID | MountFlags::NODEV,
        "",
    )
}

/// Copies Home Manager's merged `home-files` derivation (what it would symlink into `$HOME`).
#[context("materializing the home-files tree from {} into {}", home_files.display(), paths.lower.display())]
pub fn materialize_home_files(
    paths: &NythPaths,
    home_files: &Path,
    uid: Uid,
    gid: Gid,
) -> eros::Result<(), (io::Error, Errno)> {
    copy_tree_dereferenced(home_files, &paths.lower).into_union()?;
    chown_tree(&paths.lower, uid, gid)
}

/// Recursively copies `source` into `destination`, following symlinks
fn copy_tree_dereferenced(source: &Path, destination: &Path) -> io::Result<()> {
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
// No `#[context]`: this recurses, and the attribute would stack one context frame per level of directory depth
fn chown_tree(path: &Path, uid: Uid, gid: Gid) -> eros::Result<(), (io::Error, Errno)> {
    set_ownership(path, uid, gid).widen()?;

    if path.is_dir() {
        let entries = fs::read_dir(path)
            .with_user_context(|| format!("listing {}", path.display()))
            .widen()?;
        for entry in entries {
            chown_tree(&entry.into_union()?.path(), uid, gid)?;
        }
    }
    Ok(())
}

#[context("mounting the overlay over {}", target.display())]
pub fn mount_overlay(
    paths: &NythPaths,
    target: &Path,
) -> eros::Result<(), (OverlayUnsupported, Errno)> {
    let fs_fd = open_overlay_fs()?;

    set_lowerdir(&fs_fd, &paths.lower, &paths.home_snapshot).widen()?;
    set_dir_option(&fs_fd, "upperdir", &paths.upper).widen()?;
    set_dir_option(&fs_fd, "workdir", &paths.work).widen()?;
    fsconfig_create(&fs_fd).into_union()?;

    let mount_fd = fsmount(
        &fs_fd,
        FsMountFlags::FSMOUNT_CLOEXEC,
        MountAttrFlags::empty(),
    )
    .into_union()?;

    move_mount(
        &mount_fd,
        "",
        CWD,
        target,
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
    )
    .into_union()?;
    Ok(())
}

/// ENOSYS = no new mount API (kernel < 5.2); EOPNOTSUPP/ENODEV = overlay module not loaded
fn open_overlay_fs() -> eros::Result<OwnedFd, (OverlayUnsupported, Errno)> {
    fsopen("overlay", FsOpenFlags::FSOPEN_CLOEXEC).map_err(|e| match e {
        Errno::NOSYS | Errno::OPNOTSUPP | Errno::NODEV => {
            ErrorUnion::new(OverlayUnsupported { errno: e })
        }
        other => ErrorUnion::new(other),
    })
}

#[context("setting lowerdir to {}:{}", lower.display(), home_snapshot.display())]
fn set_lowerdir(fs_fd: &OwnedFd, lower: &Path, home_snapshot: &Path) -> eros::Result<(), (Errno,)> {
    let mut value = lower.as_os_str().as_bytes().to_vec();
    value.push(b':');
    value.extend_from_slice(home_snapshot.as_os_str().as_bytes());
    let value = OsString::from_vec(value);

    fsconfig_set_string(fs_fd, "lowerdir", value.as_os_str())?;
    Ok(())
}

#[context("setting {} to {}", key, dir.display())]
fn set_dir_option(fs_fd: &OwnedFd, key: &str, dir: &Path) -> eros::Result<(), (Errno,)> {
    fsconfig_set_string(fs_fd, key, dir.as_os_str())?;
    Ok(())
}

/// Unmounts the overlay at `target` and the read-only home snapshot underneath it
pub fn unmount_overlay_and_snapshot(
    target: &Path,
    paths: &NythPaths,
) -> eros::Result<(), (Errno,)> {
    unmount_one(target)?;
    unmount_one(&paths.home_snapshot)
}

/// Additionally tears down the persistent tmpfs itself (`nyth unmount --purge`): `upper`/`work` go with it
pub fn unmount_persistent_tmpfs(paths: &NythPaths) -> eros::Result<(), (Errno,)> {
    unmount_one(&paths.root)
}

#[context("unmounting {}", target.display())]
fn unmount_one(target: &Path) -> eros::Result<(), (Errno,)> {
    unmount(target, UnmountFlags::empty())?;
    Ok(())
}

/// `chown`s `path`: `upper`/`work` are created by root but must be writable by the target user
#[context("setting ownership of {}", path.display())]
pub fn set_ownership(path: &Path, uid: Uid, gid: Gid) -> eros::Result<(), (Errno,)> {
    let owner = rustix::fs::Uid::from_raw(uid.as_raw());
    let group = rustix::fs::Gid::from_raw(gid.as_raw());
    rustix::fs::chown(path, Some(owner), Some(group))?;
    Ok(())
}
