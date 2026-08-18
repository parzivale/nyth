mod support;

use std::path::PathBuf;

use nyth::cli::status::{
    DotfilesRepo, PendingChange, UpperEntry, diff_upper_against_repo, nyth_status,
};
use nyth::config::RelativeHomePath;
use support::Workspace;

fn watched(target: &str) -> RelativeHomePath {
    RelativeHomePath::new(target).expect("valid relative path")
}

fn repo_backed_only(root: PathBuf, paths: Vec<RelativeHomePath>) -> DotfilesRepo {
    DotfilesRepo::new(root, paths, Vec::new())
}

#[test]
fn diff_marks_repo_backed_paths_as_repo_backed() {
    let repo = repo_backed_only(PathBuf::from("/unused"), vec![watched(".gitconfig")]);
    let entries = vec![UpperEntry {
        relative_path: PathBuf::from(".gitconfig"),
    }];

    let changes = diff_upper_against_repo(&entries, &repo);

    assert_eq!(
        changes,
        vec![PendingChange::RepoBacked {
            relative_path: PathBuf::from(".gitconfig"),
        }]
    );
}

// Nested file under a watched dir still counts as that watched path
#[test]
fn diff_marks_nested_file_under_directory_watched_path_as_repo_backed() {
    let repo = repo_backed_only(PathBuf::from("/unused"), vec![watched(".config/hypr")]);
    let entries = vec![UpperEntry {
        relative_path: PathBuf::from(".config/hypr/hyprland.conf"),
    }];

    let changes = diff_upper_against_repo(&entries, &repo);

    assert_eq!(
        changes,
        vec![PendingChange::RepoBacked {
            relative_path: PathBuf::from(".config/hypr/hyprland.conf"),
        }]
    );
}

// Watched but rendered by a programs.* module: no repo source file
#[test]
fn diff_marks_generated_paths_as_generated_not_repo_backed() {
    let repo = DotfilesRepo::new(
        PathBuf::from("/unused"),
        vec![watched(".zshrc")],
        vec![watched(".gitconfig")],
    );
    let entries = vec![UpperEntry {
        relative_path: PathBuf::from(".gitconfig"),
    }];

    let changes = diff_upper_against_repo(&entries, &repo);

    assert_eq!(
        changes,
        vec![PendingChange::Generated {
            relative_path: PathBuf::from(".gitconfig"),
        }]
    );
}

#[test]
fn diff_marks_unwatched_paths_as_untracked() {
    let repo = repo_backed_only(PathBuf::from("/unused"), vec![watched(".gitconfig")]);
    let entries = vec![UpperEntry {
        relative_path: PathBuf::from(".config/random-app/state.db"),
    }];

    let changes = diff_upper_against_repo(&entries, &repo);

    assert_eq!(
        changes,
        vec![PendingChange::Untracked {
            relative_path: PathBuf::from(".config/random-app/state.db"),
        }]
    );
}

#[test]
fn diff_is_pure_no_watched_paths_no_entries_no_changes() {
    let repo = repo_backed_only(PathBuf::from("/unused"), vec![]);
    assert_eq!(diff_upper_against_repo(&[], &repo), vec![]);
}

#[test]
fn nyth_status_walks_upper_recursively_and_classifies_all_three_states() {
    let ws = Workspace::new("status");
    let paths = ws.paths();

    ws.write(
        "state/upper/.config/hypr/hyprland.conf",
        "monitor=,preferred,auto,1",
    );
    ws.write("state/upper/.gitconfig", "[user]\nname = test");
    ws.write("state/upper/random-state.db", "");

    let repo = DotfilesRepo::new(
        ws.root.join("dotfiles"),
        vec![watched(".config/hypr")],
        vec![watched(".gitconfig")],
    );

    let mut changes = nyth_status(&paths, &repo).expect("status should succeed");
    changes.sort_by_key(|c| match c {
        PendingChange::RepoBacked { relative_path } => relative_path.clone(),
        PendingChange::Generated { relative_path } => relative_path.clone(),
        PendingChange::Untracked { relative_path } => relative_path.clone(),
    });

    assert_eq!(
        changes,
        vec![
            PendingChange::RepoBacked {
                relative_path: PathBuf::from(".config/hypr/hyprland.conf"),
            },
            PendingChange::Generated {
                relative_path: PathBuf::from(".gitconfig"),
            },
            PendingChange::Untracked {
                relative_path: PathBuf::from("random-state.db"),
            },
        ]
    );
}
