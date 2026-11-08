//! Operations in progress (merge, rebase, `git am`, cherry-pick, revert,
//! bisect), detected from the state files Git keeps in the per-worktree Git
//! directory. These are the files `git status` itself reads; Diffkit only
//! checks whether they exist and reads a few small ones. No Git process
//! runs and nothing is written.
//!
//! In a linked worktree, `git_dir` is that worktree's own directory
//! (`<common>/worktrees/<name>`), so an operation in another worktree is
//! never reported here.
//!
//! A file that exists but cannot be read leaves the matching detail (such
//! as a rebase step) unknown; the operation itself is still reported.

use crate::types::repository::{ObjectId, Operation, RebaseBackend};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// State files are tiny; anything larger is not trusted.
const MAX_STATE_FILE: u64 = 4096;

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// First line of a small state file, trimmed; `None` if missing, unreadable,
/// too large, or empty.
fn read_line(p: &Path) -> Option<String> {
    let f = fs::File::open(p).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_STATE_FILE + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_STATE_FILE {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn read_number(p: &Path) -> Option<u64> {
    read_line(p)?.parse().ok()
}

fn step(dir: &Path, current: &str, total: &str) -> Option<(u64, u64)> {
    Some((
        read_number(&dir.join(current))?,
        read_number(&dir.join(total))?,
    ))
}

/// `head-name` holds `refs/heads/<branch>` or `detached HEAD`.
fn branch(dir: &Path) -> Option<String> {
    read_line(&dir.join("head-name")).filter(|n| n != "detached HEAD")
}

fn onto(dir: &Path) -> Option<ObjectId> {
    read_line(&dir.join("onto")).and_then(|s| ObjectId::parse(s.as_bytes()))
}

/// The first command of a sequencer todo list (`pick`, `revert`, …).
fn sequencer_command(git_dir: &Path) -> Option<String> {
    let line = read_line(&git_dir.join("sequencer").join("todo"))?;
    line.split_whitespace().next().map(str::to_owned)
}

pub fn detect(git_dir: &Path) -> Vec<Operation> {
    let path = |name: &str| -> PathBuf { git_dir.join(name) };
    let mut ops = Vec::new();
    if exists(&path("MERGE_HEAD")) {
        ops.push(Operation::Merge);
    }
    let merge_dir = path("rebase-merge");
    let apply_dir = path("rebase-apply");
    if exists(&merge_dir) {
        ops.push(Operation::Rebase {
            backend: RebaseBackend::Merge,
            step: step(&merge_dir, "msgnum", "end"),
            branch: branch(&merge_dir),
            onto: onto(&merge_dir),
        });
    } else if exists(&apply_dir) {
        let step = step(&apply_dir, "next", "last");
        if exists(&apply_dir.join("applying")) {
            ops.push(Operation::ApplyMailbox { step });
        } else {
            ops.push(Operation::Rebase {
                backend: RebaseBackend::Apply,
                step,
                branch: branch(&apply_dir),
                onto: onto(&apply_dir),
            });
        }
    }
    let sequencer = sequencer_command(git_dir);
    let sequencer_is = |cmds: &[&str]| sequencer.as_deref().is_some_and(|c| cmds.contains(&c));
    if exists(&path("CHERRY_PICK_HEAD")) || sequencer_is(&["pick", "p"]) {
        ops.push(Operation::CherryPick);
    }
    if exists(&path("REVERT_HEAD")) || sequencer_is(&["revert"]) {
        ops.push(Operation::Revert);
    }
    if exists(&path("BISECT_LOG")) {
        ops.push(Operation::Bisect {
            start: read_line(&path("BISECT_START")),
        });
    }
    ops
}

impl Operation {
    /// Short label for headers, for example `REBASING 2/5`.
    pub fn label(&self) -> String {
        let with_step = |name: &str, step: &Option<(u64, u64)>| match step {
            Some((n, total)) => format!("{name} {n}/{total}"),
            None => name.to_owned(),
        };
        match self {
            Self::Merge => "MERGING".into(),
            Self::Rebase { step, .. } => with_step("REBASING", step),
            Self::ApplyMailbox { step } => with_step("APPLYING PATCHES (am)", step),
            Self::CherryPick => "CHERRY-PICKING".into(),
            Self::Revert => "REVERTING".into(),
            Self::Bisect { .. } => "BISECTING".into(),
        }
    }

    /// One sentence for reports and help text.
    pub fn description(&self) -> String {
        match self {
            Self::Merge => "A merge is in progress (MERGE_HEAD exists).".into(),
            Self::Rebase {
                backend,
                step,
                branch,
                onto,
            } => {
                let mut s = String::from(match backend {
                    RebaseBackend::Merge => "A rebase is in progress",
                    RebaseBackend::Apply => "A rebase (apply backend) is in progress",
                });
                if let Some(b) = branch {
                    s.push_str(&format!(
                        " on {}",
                        b.strip_prefix("refs/heads/").unwrap_or(b)
                    ));
                }
                if let Some(o) = onto {
                    s.push_str(&format!(" onto {}", &o.as_str()[..12]));
                }
                if let Some((n, total)) = step {
                    s.push_str(&format!(", step {n} of {total}"));
                }
                s.push('.');
                s
            }
            Self::ApplyMailbox { step } => match step {
                Some((n, total)) => format!("git am is applying patch {n} of {total}."),
                None => "git am is applying patches.".into(),
            },
            Self::CherryPick => "A cherry-pick is in progress.".into(),
            Self::Revert => "A revert is in progress.".into(),
            Self::Bisect { start } => match start {
                Some(s) => format!("A bisect is in progress (started from {s})."),
                None => "A bisect is in progress.".into(),
            },
        }
    }
}
