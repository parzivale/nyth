use std::path::{Component, Path, PathBuf};

use eros::{ErrorUnion, StrError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeHomePath(PathBuf);

impl RelativeHomePath {
    pub fn new(path: impl Into<PathBuf>) -> eros::Result<Self, (StrError,)> {
        let path = path.into();

        if path.is_absolute() {
            return Err(ErrorUnion::new(StrError::Owned(format!(
                "target path {} must be relative to $HOME, not absolute",
                path.display()
            ))));
        }

        if path.components().any(|c| c == Component::ParentDir) {
            return Err(ErrorUnion::new(StrError::Owned(format!(
                "target path {} contains '..' and would escape $HOME",
                path.display()
            ))));
        }

        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}
