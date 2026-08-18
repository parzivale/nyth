mod support;

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;

use eros::{ErrorUnion, TypeSet};
use nyth::sys::overlay::{
    materialize_home_files, mount_overlay, provision_persistent_tmpfs, unmount_persistent_tmpfs,
};
use nyth::sys::paths::NythPaths;

/// A uid/gid that's neither root nor whatever nyth itself provisioned things as
const FAKE_TARGET_UID: nix::unistd::Uid = nix::unistd::Uid::from_raw(6553);
const FAKE_TARGET_GID: nix::unistd::Gid = nix::unistd::Gid::from_raw(6553);

/// True if EPERM/EACCES: this process isn't running as real root.
/// Checks both `io::Error` (mkdir) and rustix errno (mount) arms.
fn is_permission_denied<E: TypeSet>(e: &ErrorUnion<E>) -> bool {
    e.downcast_inner_ref::<rustix::io::Errno>()
        .is_some_and(|errno| {
            *errno == rustix::io::Errno::PERM || *errno == rustix::io::Errno::ACCESS
        })
        || e.downcast_inner_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
}

fn umount_path(path: &Path) {
    let c = CString::new(path.as_os_str().as_bytes()).expect("path has no interior NUL");
    unsafe {
        libc::umount2(c.as_ptr(), 0);
    }
}

#[test]
fn mount_overlay_merges_lower_and_allows_writes() {
    support::run_in_fork(run_in_child);
}

fn run_in_child() -> i32 {
    let name = format!("nyth-test-overlay-{}", std::process::id());
    let paths = NythPaths::for_user(&name);

    if let Err(e) = provision_persistent_tmpfs(&paths, FAKE_TARGET_UID, FAKE_TARGET_GID) {
        if is_permission_denied(&e) {
            return 0;
        }
        eprintln!("provision_persistent_tmpfs failed: {e:?}");
        return 1;
    }

    match fs::metadata(&paths.root) {
        Ok(metadata)
            if metadata.uid() == FAKE_TARGET_UID.as_raw()
                && metadata.gid() == FAKE_TARGET_GID.as_raw() => {}
        Ok(metadata) => {
            eprintln!(
                "paths.root not chowned to target user (uid={}, gid={})",
                metadata.uid(),
                metadata.gid()
            );
            let _ = unmount_persistent_tmpfs(&paths);
            return 10;
        }
        Err(e) => {
            eprintln!("stat on paths.root failed: {e}");
            let _ = unmount_persistent_tmpfs(&paths);
            return 11;
        }
    }

    if let Err(e) = fs::write(paths.lower.join("testfile"), b"from-lower") {
        eprintln!("seed lowerdir failed: {e}");
        let _ = unmount_persistent_tmpfs(&paths);
        return 2;
    }

    let target =
        std::env::temp_dir().join(format!("nyth-test-overlay-target-{}", std::process::id()));
    let _ = fs::remove_dir_all(&target);
    if fs::create_dir_all(&target).is_err() {
        eprintln!("create target dir failed");
        let _ = unmount_persistent_tmpfs(&paths);
        return 3;
    }

    if let Err(e) = mount_overlay(&paths, &target) {
        eprintln!("mount_overlay failed: {e:?}");
        let _ = unmount_persistent_tmpfs(&paths);
        let _ = fs::remove_dir_all(&target);
        return 4;
    }

    let result = check_overlay_contents(&target, &paths.upper);

    umount_path(&target);
    let _ = unmount_persistent_tmpfs(&paths);
    let _ = fs::remove_dir_all(&target);

    result
}

fn check_overlay_contents(target: &Path, upper: &Path) -> i32 {
    let seen = match fs::read(target.join("testfile")) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("reading through overlay failed: {e}");
            return 5;
        }
    };
    if seen != b"from-lower" {
        eprintln!("lowerdir content did not surface through overlay");
        return 6;
    }

    if let Err(e) = fs::write(target.join("newfile"), b"from-mount") {
        eprintln!("write through overlay failed: {e}");
        return 7;
    }

    match fs::read(upper.join("newfile")) {
        Ok(bytes) if bytes == b"from-mount" => 0,
        Ok(_) => {
            eprintln!("upperdir file had unexpected content");
            8
        }
        Err(e) => {
            eprintln!("write did not land in upperdir: {e}");
            9
        }
    }
}

#[test]
fn materialize_home_files_dereferences_symlinks_and_chowns_to_target_user() {
    support::run_in_fork(run_materialize_in_child);
}

fn run_materialize_in_child() -> i32 {
    let name = format!("nyth-test-materialize-{}", std::process::id());
    let paths = NythPaths::for_user(&name);

    if let Err(e) = provision_persistent_tmpfs(&paths, FAKE_TARGET_UID, FAKE_TARGET_GID) {
        if is_permission_denied(&e) {
            return 0;
        }
        eprintln!("provision_persistent_tmpfs failed: {e:?}");
        return 1;
    }

    // Fake `home-files`: a dir with a real file and a subdir reached via symlink
    let fake_home_files = paths.root.join("fake-home-files");
    let fake_store_dir = paths.root.join("fake-store-dir");
    if fs::create_dir_all(&fake_home_files).is_err() || fs::create_dir_all(&fake_store_dir).is_err()
    {
        eprintln!("failed to set up fake home-files tree");
        let _ = unmount_persistent_tmpfs(&paths);
        return 2;
    }
    if fs::write(fake_home_files.join(".gitconfig"), b"from-nix-store").is_err() {
        eprintln!("failed to seed fake home-files file");
        let _ = unmount_persistent_tmpfs(&paths);
        return 2;
    }
    if fs::write(
        fake_store_dir.join("hyprland.conf"),
        b"from-nested-store-dir",
    )
    .is_err()
    {
        eprintln!("failed to seed fake nested store dir");
        let _ = unmount_persistent_tmpfs(&paths);
        return 2;
    }
    if symlink(&fake_store_dir, fake_home_files.join("hypr")).is_err() {
        eprintln!("failed to symlink fake directory the way home-manager would");
        let _ = unmount_persistent_tmpfs(&paths);
        return 3;
    }

    if let Err(e) =
        materialize_home_files(&paths, &fake_home_files, FAKE_TARGET_UID, FAKE_TARGET_GID)
    {
        eprintln!("materialize_home_files failed: {e:?}");
        let _ = unmount_persistent_tmpfs(&paths);
        return 4;
    }

    let result = check_materialized_lower(&paths.lower);
    let _ = unmount_persistent_tmpfs(&paths);
    result
}

fn check_materialized_lower(lower: &Path) -> i32 {
    // Real content copied in, owned by the target user, not a bind mount or symlink.
    let file_entry = lower.join(".gitconfig");
    match fs::symlink_metadata(&file_entry) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            eprintln!("lower/.gitconfig is still a symlink");
            return 5;
        }
        Ok(metadata) => {
            if metadata.uid() != FAKE_TARGET_UID.as_raw()
                || metadata.gid() != FAKE_TARGET_GID.as_raw()
            {
                eprintln!(
                    "lower/.gitconfig not chowned to target user (uid={}, gid={})",
                    metadata.uid(),
                    metadata.gid()
                );
                return 6;
            }
        }
        Err(e) => {
            eprintln!("stat on lower/.gitconfig failed: {e}");
            return 7;
        }
    }
    match fs::read(&file_entry) {
        Ok(bytes) if bytes == b"from-nix-store" => {}
        Ok(_) => {
            eprintln!("lower/.gitconfig content did not match the source");
            return 8;
        }
        Err(e) => {
            eprintln!("reading lower/.gitconfig failed: {e}");
            return 9;
        }
    }

    // A dir reached only via symlink in the source must come through as real content too.
    let nested_entry = lower.join("hypr").join("hyprland.conf");
    match fs::symlink_metadata(lower.join("hypr")) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            eprintln!("lower/hypr is still a symlink");
            return 10;
        }
        Ok(metadata) => {
            if metadata.uid() != FAKE_TARGET_UID.as_raw()
                || metadata.gid() != FAKE_TARGET_GID.as_raw()
            {
                eprintln!("lower/hypr not chowned to target user");
                return 11;
            }
        }
        Err(e) => {
            eprintln!("stat on lower/hypr failed: {e}");
            return 12;
        }
    }
    match fs::read(&nested_entry) {
        Ok(bytes) if bytes == b"from-nested-store-dir" => 0,
        Ok(_) => {
            eprintln!("lower/hypr/hyprland.conf content did not match the source");
            13
        }
        Err(e) => {
            eprintln!("reading lower/hypr/hyprland.conf failed: {e}");
            14
        }
    }
}
