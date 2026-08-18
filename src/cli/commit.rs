use std::fmt;
use std::io;
use std::path::PathBuf;

use eros::{Context, ErrorUnion, ReshapeUnion};

use crate::cli::status::{DotfilesRepo, PendingChange, RepoArgs, nyth_status};
use crate::config::RelativeHomePath;
use crate::sys::paths::NythPaths;

/// Which pending changes `nyth commit` should actually write back to the repo
#[derive(Debug, Clone)]
pub enum CommitSelection {
    All,
    WatchedPaths(Vec<RelativeHomePath>),
}

#[derive(Debug, Clone)]
pub struct CommitReport {
    /// Repo paths that got written, in the order they were applied
    pub applied: Vec<PathBuf>,
}

/// A change that was refused outright, as opposed to one that failed while being applied.
/// Carries the path and the reason itself, so callers that `narrow()` it back out of
/// `apply_commit` have the whole story without any attached context.
#[derive(Debug)]
pub enum NotCommittable {
    /// Rendered by a `programs.*` module from Nix options; no source file in the repo to write to
    Generated { path: PathBuf },
    /// Not managed by Home Manager at all
    Untracked { path: PathBuf },
}

impl fmt::Display for NotCommittable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generated { path } => write!(
                f,
                "{} is rendered by a programs.* module, not backed by a repo file — nothing to commit it to",
                path.display()
            ),
            Self::Untracked { path } => write!(
                f,
                "{} is not managed by Home Manager, cannot commit an untracked path",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NotCommittable {}

/// Which pending changes match `selection`. `Untracked` changes are never selected, regardless of the filter
pub fn select_changes_to_apply(
    pending: &[PendingChange],
    selection: &CommitSelection,
) -> Vec<PendingChange> {
    pending
        .iter()
        .filter(|change| match change {
            PendingChange::RepoBacked { relative_path } => match selection {
                CommitSelection::All => true,
                CommitSelection::WatchedPaths(paths) => paths.iter().any(|watched| {
                    let target = watched.as_path();
                    relative_path == target || relative_path.starts_with(target)
                }),
            },
            PendingChange::Generated { .. } | PendingChange::Untracked { .. } => false,
        })
        .cloned()
        .collect()
}

/// Builds identity-scoped paths and the repo from `--for-user`/`--repo-*` args, then commits
/// Thin wrapper around `commit_into`.
pub fn commit(args: &RepoArgs) -> eros::Result<CommitReport> {
    let paths = args.paths();
    let repo = args.clone().into_repo();
    commit_into(&repo, &paths)
}

pub fn commit_into(repo: &DotfilesRepo, paths: &NythPaths) -> eros::Result<CommitReport> {
    let pending = nyth_status(paths, repo)?;
    let selected = select_changes_to_apply(&pending, &CommitSelection::All);

    Ok(apply_commit(&selected, paths, repo)?)
}

/// Writes each already-selected change back to the repo, at the same $HOME-relative path it changed at: the repo mirrors $HOME under `repo.root`
///
/// `NotCommittable` stays a separate arm of the union rather than being erased, because
/// callers that pass an unfiltered list (`select_changes_to_apply` filters these out) can
/// `narrow()` it out and skip those paths instead of failing the whole run.
pub fn apply_commit(
    selected: &[PendingChange],
    paths: &NythPaths,
    repo: &DotfilesRepo,
) -> eros::Result<CommitReport, (NotCommittable, io::Error)> {
    let mut applied = Vec::with_capacity(selected.len());

    for change in selected {
        applied.push(apply_one_change(paths, repo, change)?);
    }

    Ok(CommitReport { applied })
}

fn apply_one_change(
    paths: &NythPaths,
    repo: &DotfilesRepo,
    change: &PendingChange,
) -> eros::Result<PathBuf, (NotCommittable, io::Error)> {
    let relative_path = match change {
        PendingChange::RepoBacked { relative_path } => relative_path,
        PendingChange::Generated { relative_path } => {
            return Err(ErrorUnion::new(NotCommittable::Generated {
                path: relative_path.clone(),
            }));
        }
        PendingChange::Untracked { relative_path } => {
            return Err(ErrorUnion::new(NotCommittable::Untracked {
                path: relative_path.clone(),
            }));
        }
    };

    let source_in_upper = paths.upper.join(relative_path);
    let destination = repo.root.join(relative_path);

    crate::fs_util::copy_file_preserving_symlinks(&source_in_upper, &destination)
        .with_user_context(|| format!("committing {}", destination.display()))
        .widen()?;
    Ok(destination)
}
