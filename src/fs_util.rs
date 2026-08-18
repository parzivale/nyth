use std::fs;
use std::io;
use std::path::Path;

use eros::context;

/// Copies a single file to `destination`, creating parent dirs first.
/// Symlinks are recreated as symlinks, not dereferenced (unlike `fs::copy()`).
#[context("copying {} to {}", source.display(), destination.display())]
pub fn copy_file_preserving_symlinks(
    source: &Path,
    destination: &Path,
) -> eros::Result<(), (io::Error,)> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    let metadata = fs::symlink_metadata(source)?;

    if metadata.is_symlink() {
        let link_target = fs::read_link(source)?;
        let _ = fs::remove_file(destination);
        std::os::unix::fs::symlink(&link_target, destination)?;
    } else {
        fs::copy(source, destination)?;
    }

    Ok(())
}
