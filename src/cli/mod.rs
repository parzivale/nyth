use std::path::Path;
use std::process::ExitCode;

pub mod commit;
pub mod generated_diff;
pub mod mount;
pub mod status;
pub mod unmount;

use commit::commit;
use generated_diff::{read_generated_change, render_generated_change};
use mount::{parse_mount_args, run_mount};
use status::{PendingChange, RepoArgs, parse_repo_args, status};
use unmount::{parse_unmount_args, run_unmount};

/// Dispatches on `args[1]` (the subcommand). `args[0]` is the program name,
/// same convention as `std::env::args()`, so callers can pass that straight
/// through without stripping anything first.
pub fn run(args: &[String]) -> ExitCode {
    match args.get(1).map(String::as_str) {
        Some("mount") => run_mount_cmd(&args[2..]),
        Some("unmount") => run_unmount_cmd(&args[2..]),
        Some("status") => run_status(&args[2..]),
        Some("commit") => run_commit(&args[2..]),
        Some(other) => {
            eprintln!("nyth: unknown command '{other}'");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: nyth <mount|unmount|status|commit> [args]");
            ExitCode::FAILURE
        }
    }
}

fn run_mount_cmd(args: &[String]) -> ExitCode {
    let mount_args = match parse_mount_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("nyth mount: {e}");
            return ExitCode::FAILURE;
        }
    };

    match run_mount(&mount_args) {
        Ok(()) => {
            println!("mounted for user '{}'", mount_args.for_user);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nyth mount failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_unmount_cmd(args: &[String]) -> ExitCode {
    let unmount_args = match parse_unmount_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("nyth unmount: {e}");
            return ExitCode::FAILURE;
        }
    };

    match run_unmount(&unmount_args) {
        Ok(()) => {
            println!("unmounted for user '{}'", unmount_args.for_user);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nyth unmount failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_status(args: &[String]) -> ExitCode {
    let repo_args = match parse_repo_args(args) {
        Ok(repo_args) => repo_args,
        Err(e) => {
            eprintln!("nyth status: {e}");
            return ExitCode::FAILURE;
        }
    };

    let changes = match status(&repo_args) {
        Ok(changes) => changes,
        Err(e) => {
            eprintln!("nyth status failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    if changes.is_empty() {
        println!("nothing to commit");
        return ExitCode::SUCCESS;
    }

    for change in &changes {
        match change {
            PendingChange::Generated { relative_path } => {
                print_generated_change(&repo_args, relative_path);
            }
            other => println!("{other:?}"),
        }
    }

    ExitCode::SUCCESS
}

/// `Generated` changes are diffed against the target user's *live* $HOME (what Home Manager currently has active there), not the repo
fn print_generated_change(repo_args: &RepoArgs, relative_path: &Path) {
    if let Ok(Some(user)) = nix::unistd::User::from_name(&repo_args.for_user) {
        let upper = repo_args.paths().upper;

        match read_generated_change(&user.dir, &upper, relative_path) {
            Ok(change) => print!("{}", render_generated_change(&change)),
            Err(e) => println!("Generated {{ relative_path: {relative_path:?} }} ({e})"),
        }
    } else {
        println!(
            "Generated {{ relative_path: {relative_path:?} }} (couldn't resolve identity, showing raw path only)"
        );
    }
}

fn run_commit(args: &[String]) -> ExitCode {
    let repo_args = match parse_repo_args(args) {
        Ok(repo_args) => repo_args,
        Err(e) => {
            eprintln!("nyth commit: {e}");
            return ExitCode::FAILURE;
        }
    };

    match commit(&repo_args) {
        Ok(report) if report.applied.is_empty() => {
            println!("nothing to commit");
            ExitCode::SUCCESS
        }
        Ok(report) => {
            for path in report.applied {
                println!("committed {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nyth commit failed: {e}");
            ExitCode::FAILURE
        }
    }
}
