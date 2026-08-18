mod support;

use std::fs;

use nyth::cli::commit::commit_into;
use nyth::cli::status::DotfilesRepo;
use nyth::config::RelativeHomePath;
use support::Workspace;

fn watched(target: &str) -> RelativeHomePath {
    RelativeHomePath::new(target).expect("valid relative path")
}

fn repo_backed_only(root: std::path::PathBuf, paths: Vec<RelativeHomePath>) -> DotfilesRepo {
    DotfilesRepo::new(root, paths, Vec::new())
}

#[test]
fn commit_writes_repo_backed_file_back_to_repo_at_the_same_relative_path() {
    let ws = Workspace::new("basic");
    ws.write("dotfiles/.gitconfig", "[user]\nname = old");
    let repo = repo_backed_only(ws.root.join("dotfiles"), vec![watched(".gitconfig")]);
    let paths = ws.paths();
    ws.write("state/upper/.gitconfig", "[user]\nname = new");

    let report = commit_into(&repo, &paths).expect("commit should succeed");

    assert_eq!(report.applied, vec![ws.root.join("dotfiles/.gitconfig")]);
    let written = fs::read_to_string(ws.root.join("dotfiles/.gitconfig")).expect("read repo file");
    assert_eq!(written, "[user]\nname = new");
}

#[test]
fn commit_ignores_untracked_files() {
    let ws = Workspace::new("untracked");
    let repo = repo_backed_only(ws.root.join("dotfiles"), vec![]);
    let paths = ws.paths();
    ws.write("state/upper/random-state.db", "");

    let report = commit_into(&repo, &paths).expect("commit should succeed");

    assert!(report.applied.is_empty());
}

// Generated configs must never be silently written into the repo at a nonexistent path
#[test]
fn commit_never_writes_generated_configs_anywhere() {
    let ws = Workspace::new("generated");
    let repo = DotfilesRepo::new(
        ws.root.join("dotfiles"),
        Vec::new(),
        vec![watched(".gitconfig")],
    );
    let paths = ws.paths();
    ws.write("state/upper/.gitconfig", "[user]\nname = edited-live");

    let report = commit_into(&repo, &paths).expect("commit should succeed");

    assert!(report.applied.is_empty());
    assert!(!ws.root.join("dotfiles/.gitconfig").exists());
}

#[test]
fn commit_writes_nested_directory_watched_path_file_to_right_subpath() {
    let ws = Workspace::new("nested");
    ws.write("dotfiles/.config/hypr/hyprland.conf", "monitor=old");
    let repo = repo_backed_only(ws.root.join("dotfiles"), vec![watched(".config/hypr")]);
    let paths = ws.paths();
    ws.write("state/upper/.config/hypr/hyprland.conf", "monitor=new");

    let report = commit_into(&repo, &paths).expect("commit should succeed");

    assert_eq!(
        report.applied,
        vec![ws.root.join("dotfiles/.config/hypr/hyprland.conf")]
    );
    let written = fs::read_to_string(ws.root.join("dotfiles/.config/hypr/hyprland.conf"))
        .expect("read repo file");
    assert_eq!(written, "monitor=new");
}

#[test]
fn commit_with_nothing_in_upper_applies_nothing() {
    let ws = Workspace::new("empty");
    ws.write("dotfiles/.gitconfig", "[user]\nname = untouched");
    let repo = repo_backed_only(ws.root.join("dotfiles"), vec![watched(".gitconfig")]);
    fs::create_dir_all(ws.paths().upper).expect("create empty upper dir");

    let report = commit_into(&repo, &ws.paths()).expect("commit should succeed");

    assert!(report.applied.is_empty());
    let untouched =
        fs::read_to_string(ws.root.join("dotfiles/.gitconfig")).expect("read repo file");
    assert_eq!(untouched, "[user]\nname = untouched");
}
