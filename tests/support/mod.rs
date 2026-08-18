use std::fs;
use std::path::PathBuf;

use nyth::sys::paths::NythPaths;

/// A throwaway /tmp dir, torn down on drop (even on panic).
/// For tests needing plain file I/O with no mounting involved.
#[allow(dead_code)]
pub struct Workspace {
    pub root: PathBuf,
}

#[allow(dead_code)]
impl Workspace {
    /// `name` must be unique among tests running concurrently in this binary.
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("nyth-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create workspace root");
        Self { root }
    }

    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).expect("create parent dirs");
        fs::write(path, contents).expect("write workspace file");
    }

    /// Full `NythPaths` layout rooted in this workspace, with `upper`/`work` too.
    pub fn paths(&self) -> NythPaths {
        let root = self.root.join("state");
        NythPaths {
            lower: root.join("lower"),
            home_snapshot: root.join("home-snapshot"),
            upper: root.join("upper"),
            work: root.join("work"),
            root,
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Forks and runs `child_fn` in the child, asserting it exits with code 0.
/// For tests needing a fresh process for root-only syscalls (mount/chown).
/// `child_fn` exits from inside this function; it never returns on success.
pub fn run_in_fork(child_fn: impl FnOnce() -> i32) {
    match unsafe { libc::fork() } {
        -1 => panic!("fork failed"),
        0 => std::process::exit(child_fn()),
        child_pid => {
            let mut status = 0;
            unsafe { libc::waitpid(child_pid, &mut status, 0) };
            assert!(libc::WIFEXITED(status), "child did not exit normally");
            assert_eq!(
                libc::WEXITSTATUS(status),
                0,
                "see child stderr above for which step failed"
            );
        }
    }
}
