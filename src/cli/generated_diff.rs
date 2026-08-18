use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use eros::context;

/// One line-level difference in a Generated config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineDiff {
    Added(String),
    Removed(String),
    Changed { before: String, after: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedChange {
    pub relative_path: PathBuf,
    pub lines: Vec<LineDiff>,
}

/// Pure: diffs `before` (currently active) against `after` (upper).
pub fn diff_generated_config(before: &str, after: &str) -> Vec<LineDiff> {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    pair_adjacent_replacements(line_diff_ops(&before_lines, &after_lines))
}

enum RawOp<'a> {
    Equal,
    Removed(&'a str),
    Added(&'a str),
}

// LCS-table line diff, O(n*m) - fine for dotfiles-sized configs.
fn line_diff_ops<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<RawOp<'a>> {
    let (n, m) = (before.len(), after.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if before[i] == after[j] {
            ops.push(RawOp::Equal);
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(RawOp::Removed(before[i]));
            i += 1;
        } else {
            ops.push(RawOp::Added(after[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push(RawOp::Removed(before[i]));
        i += 1;
    }
    while j < m {
        ops.push(RawOp::Added(after[j]));
        j += 1;
    }
    ops
}

fn pair_adjacent_replacements(ops: Vec<RawOp<'_>>) -> Vec<LineDiff> {
    let mut out = Vec::new();
    let mut iter = ops.into_iter().peekable();

    while let Some(op) = iter.next() {
        match op {
            RawOp::Equal => {}
            RawOp::Removed(before_line) => {
                if matches!(iter.peek(), Some(RawOp::Added(_))) {
                    let Some(RawOp::Added(after_line)) = iter.next() else {
                        unreachable!()
                    };
                    out.push(LineDiff::Changed {
                        before: before_line.to_string(),
                        after: after_line.to_string(),
                    });
                } else {
                    out.push(LineDiff::Removed(before_line.to_string()));
                }
            }
            RawOp::Added(line) => out.push(LineDiff::Added(line.to_string())),
        }
    }

    out
}

/// Reads both files, diffs them.
pub fn read_generated_change(
    before_root: &Path,
    upper_root: &Path,
    relative_path: &Path,
) -> eros::Result<GeneratedChange, (io::Error,)> {
    let before = read_to_string_or_empty(&before_root.join(relative_path))?;
    let after = read_to_string_or_empty(&upper_root.join(relative_path))?;

    Ok(GeneratedChange {
        relative_path: relative_path.to_path_buf(),
        lines: diff_generated_config(&before, &after),
    })
}

// Missing file = empty content, not a read error
#[context("reading {}", path.display())]
fn read_to_string_or_empty(path: &Path) -> eros::Result<String, (io::Error,)> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e)?,
    }
}

/// Plain before/now framing, not a unified diff.
pub fn render_generated_change(change: &GeneratedChange) -> String {
    let mut out = format!(
        "~/{} is rendered by a programs.* module in your Home Manager config.\n\
         Changes here will be overwritten by the next `home-manager switch`.\n\
         To keep them, move them into your Nix config:\n\n",
        change.relative_path.display()
    );

    for line in &change.lines {
        match line {
            LineDiff::Changed { before, after } => {
                out.push_str("  changed:\n");
                out.push_str(&format!("    was: {before}\n"));
                out.push_str(&format!("    now: {after}\n"));
            }
            LineDiff::Added(line) => out.push_str(&format!("  added:\n    {line}\n")),
            LineDiff::Removed(line) => out.push_str(&format!("  removed:\n    {line}\n")),
        }
    }

    out.push_str(
        "\nnyth doesn't know which Nix option this maps to: check the programs.*\n\
         options for whichever program owns this file.\n",
    );
    out
}
