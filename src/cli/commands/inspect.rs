//! `diffkit inspect`: formats a [`RepositorySnapshot`] as the plain-text
//! inspection report.
//!
//! All values come from the snapshot. Paths are displayed unambiguously:
//! names with control characters, quotes, backslashes, surrounding
//! whitespace, ` -> `, or bytes that are not UTF-8 are quoted and escaped
//! the way Git quotes paths; the snapshot itself keeps the raw path.

use crate::types::repository::{
    BranchInfo, Change, CommitTrackingInfo, ConflictStage, FileStatus, HeadState, Operation,
    RepositoryKind, RepositorySnapshot, RepositoryStatus, TrackingRelation, TrackingUnavailable,
    UpstreamInfo,
};
use crate::{git::snapshot, helpers::display::display_path};
use std::fmt::Write;

/// Inspects the repository containing the current directory and prints
/// the report.
pub fn run() -> i32 {
    super::super::finish(snapshot::inspect_current_dir().map(|s| render(&s)))
}

const RULE: &str = "------------------------------------------------------------";

pub fn render(s: &RepositorySnapshot) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{RULE}\nDIFFKIT - REPOSITORY INSPECTOR\n{RULE}");
    repository(&mut out, s);
    branch(&mut out, &s.branch);
    operations(&mut out, &s.operations);
    upstream(&mut out, s);
    working_tree(&mut out, &s.status);
    files(&mut out, &s.status);
    let _ = writeln!(out, "\n{RULE}\nEND OF REPORT\n{RULE}");
    out
}

fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n{title}");
}

fn field(out: &mut String, label: &str, lines: &[&str]) {
    let _ = writeln!(out, "\n{label}:");
    for line in lines {
        let _ = writeln!(out, "  {line}");
    }
}

fn repository(out: &mut String, s: &RepositorySnapshot) {
    let r = &s.repository;
    section(out, "REPOSITORY");
    field(out, "Root", &[&display_path(&r.root)]);
    field(
        out,
        "Current directory",
        &[&display_path(&r.current_directory)],
    );
    field(out, "Git directory", &[&display_path(&r.git_dir)]);
    field(
        out,
        "Common Git directory",
        &[&display_path(&r.common_git_dir)],
    );
    let kind = match r.kind {
        RepositoryKind::Standard => "Standard repository",
        RepositoryKind::LinkedWorktree => {
            "Linked worktree (its Git directory is inside the common Git directory)"
        }
    };
    field(out, "Repository type", &[kind]);
    if r.shallow {
        field(
            out,
            "History",
            &["Shallow clone: commit history is truncated in this repository"],
        );
    }
}

fn branch(out: &mut String, b: &BranchInfo) {
    section(out, "BRANCH");
    let (name, state) = match &b.head {
        HeadState::Branch(n) => (n.clone(), "Normal branch".to_owned()),
        HeadState::Detached => ("(none: HEAD is detached)".into(), "Detached HEAD".into()),
        HeadState::Unborn(n) => (
            n.clone(),
            match b.empty_repository {
                Some(true) => "Unborn branch: empty repository, no commits yet".into(),
                _ => "Unborn branch: no commits on this branch yet".into(),
            },
        ),
        HeadState::Unknown => (
            "(unknown)".into(),
            "Unknown: Git did not report a branch".into(),
        ),
    };
    field(out, "Current branch", &[&name]);
    let head = match &b.head_commit {
        Some(oid) => oid.to_string(),
        None => "(none: no commits yet)".into(),
    };
    field(out, "HEAD", &[&head]);
    field(out, "Branch state", &[&state]);
}

fn operations(out: &mut String, ops: &[Operation]) {
    let lines: Vec<String> = if ops.is_empty() {
        vec!["None".into()]
    } else {
        ops.iter().map(Operation::description).collect()
    };
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    field(out, "Operation in progress", &lines);
}

fn upstream(out: &mut String, s: &RepositorySnapshot) {
    section(out, "UPSTREAM");
    let upstream = match &s.upstream {
        UpstreamInfo::Available { name, full_ref, .. } => match full_ref {
            Some(r) => format!("{name} ({r})"),
            None => name.clone(),
        },
        UpstreamInfo::Missing { name } => {
            format!("{name} (configured, but this ref does not exist locally)")
        }
        UpstreamInfo::NotConfigured => match s.branch.name() {
            Some(b) => format!("No upstream configured for branch {b}"),
            None => "No upstream configured".into(),
        },
        UpstreamInfo::NotApplicable => "Not applicable: HEAD is not on a branch".into(),
    };
    field(out, "Upstream", &[&upstream]);
    let commit = s
        .upstream
        .commit()
        .map_or_else(|| "(unavailable)".into(), |c| c.to_string());
    field(out, "Upstream commit", &[&commit]);
    let tracking = tracking_lines(&s.commit_tracking);
    let mut lines: Vec<&str> = tracking.iter().map(String::as_str).collect();
    if s.repository.shallow && s.commit_tracking.comparison_available() {
        lines.push("Counts are limited to the history available in this shallow clone.");
    }
    field(out, "Tracking status", &lines);
    let local_branch = matches!(
        &s.upstream,
        UpstreamInfo::Available { full_ref: Some(r), .. } if r.starts_with("refs/heads/")
    );
    if local_branch {
        field(
            out,
            "Information source",
            &["Local branch reference; no remote is involved."],
        );
    } else if s.upstream.configured() {
        field(
            out,
            "Information source",
            &[
                "Locally available remote-tracking reference.",
                "Remote state has not been verified by fetching; it may be stale.",
            ],
        );
    }
}

fn tracking_lines(t: &CommitTrackingInfo) -> Vec<String> {
    let commits = |n: u64| format!("{n} commit{}", if n == 1 { "" } else { "s" });
    match *t {
        CommitTrackingInfo::Compared { ahead, behind } => {
            let summary = match TrackingRelation::of(ahead, behind) {
                TrackingRelation::UpToDate => "Up to date with the locally known upstream",
                TrackingRelation::Ahead => "Ahead of the locally known upstream",
                TrackingRelation::Behind => "Behind the locally known upstream",
                TrackingRelation::Diverged => "Diverged from the locally known upstream",
            };
            vec![
                summary.into(),
                format!("Ahead by {}", commits(ahead)),
                format!("Behind by {}", commits(behind)),
            ]
        }
        CommitTrackingInfo::Unavailable(why) => vec![format!(
            "Unavailable: {}",
            match why {
                TrackingUnavailable::NoUpstream => "no upstream is configured",
                TrackingUnavailable::DetachedHead => "HEAD is detached",
                TrackingUnavailable::UpstreamMissing =>
                    "the upstream ref is missing locally (never fetched, or deleted and pruned)",
                TrackingUnavailable::UnbornHead => "the current branch has no commits yet",
                TrackingUnavailable::NotReported => "Git did not report ahead/behind counts",
            }
        )],
    }
}

fn files_count(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

fn working_tree(out: &mut String, s: &RepositoryStatus) {
    section(out, "WORKING TREE");
    let state = if s.is_clean() {
        "Clean: no staged, unstaged, untracked, or conflicted files"
    } else {
        "Changes present"
    };
    field(out, "State", &[state]);
    for (label, n) in [
        ("Staged", s.staged_count()),
        ("Unstaged", s.unstaged_count()),
        ("Partially staged", s.partially_staged_count()),
        ("Untracked", s.untracked_count()),
        ("Conflicted", s.conflicted_count()),
        ("Deleted", s.deleted_count()),
        ("Renamed", s.renamed_count()),
        ("Copied", s.copied_count()),
    ] {
        field(out, label, &[&files_count(n)]);
    }
}

/// A section title, which files belong in it, and how each is described.
type Group = (
    &'static str,
    fn(&FileStatus) -> bool,
    fn(&FileStatus) -> String,
);

fn files(out: &mut String, s: &RepositoryStatus) {
    section(out, "FILES");
    if s.is_clean() {
        let _ = writeln!(out, "\n  (none)");
        return;
    }
    let groups: [Group; 8] = [
        (
            "STAGED (index differs from HEAD)",
            FileStatus::is_staged,
            |f| describe(f.index),
        ),
        (
            "UNSTAGED (working tree differs from index)",
            FileStatus::is_unstaged,
            |f| describe(f.worktree),
        ),
        (
            "PARTIALLY STAGED (both staged and unstaged changes)",
            FileStatus::is_partially_staged,
            |f| {
                format!(
                    "staged {}, unstaged {}",
                    describe(f.index),
                    describe(f.worktree)
                )
            },
        ),
        ("UNTRACKED", FileStatus::is_untracked, |_| {
            "untracked".into()
        }),
        ("CONFLICTED (unmerged)", FileStatus::is_conflicted, |f| {
            f.conflict.map_or("conflicted", |c| c.description()).into()
        }),
        ("DELETED", FileStatus::is_deleted, |f| {
            match (f.index == Change::Deleted, f.worktree == Change::Deleted) {
                (true, _) => "deletion staged".into(),
                _ => "deleted in working tree, not staged".into(),
            }
        }),
        ("RENAMED", FileStatus::is_renamed, |f| {
            side_note(f, Change::Renamed, "renamed")
        }),
        ("COPIED", FileStatus::is_copied, |f| {
            side_note(f, Change::Copied, "copied")
        }),
    ];
    for (title, select, describe) in groups {
        let mut entries = s.files.iter().filter(|f| select(f)).peekable();
        if entries.peek().is_none() {
            continue;
        }
        let _ = writeln!(out, "\n{title}");
        for f in entries {
            let _ = writeln!(out, "  {}  {}  ({})", xy(f), entry_path(f), describe(f));
        }
    }
    conflict_stages(out, s);
}

/// The index stages of each conflicted path, abbreviated to 12 digits.
fn conflict_stages(out: &mut String, s: &RepositoryStatus) {
    let mut entries = s
        .files
        .iter()
        .filter_map(|f| Some((f, f.stages.as_ref()?)))
        .peekable();
    if entries.peek().is_none() {
        return;
    }
    let _ = writeln!(out, "\nCONFLICT STAGES (index stages Git recorded)");
    for (f, st) in entries {
        let stage = |s: &Option<ConflictStage>| {
            s.as_ref()
                .map_or("(none)".to_string(), |s| s.oid.as_str()[..12].to_string())
        };
        let _ = writeln!(
            out,
            "  {}  base {}  ours {}  theirs {}",
            display_path(&f.path),
            stage(&st.base),
            stage(&st.ours),
            stage(&st.theirs)
        );
    }
}

fn side_note(f: &FileStatus, change: Change, verb: &str) -> String {
    let side = if f.index == change {
        "staged"
    } else {
        "in working tree"
    };
    match f.similarity {
        Some(s) => format!("{verb}, {side}, {}% similar", s.0),
        None => format!("{verb}, {side}"),
    }
}

fn describe(c: Change) -> String {
    match c {
        Change::Unmodified => "unmodified".into(),
        Change::Modified => "modified".into(),
        Change::TypeChanged => "type changed".into(),
        Change::Added => "added".into(),
        Change::Deleted => "deleted".into(),
        Change::Renamed => "renamed".into(),
        Change::Copied => "copied".into(),
        Change::Unmerged => "unmerged".into(),
        Change::Untracked => "untracked".into(),
        Change::Unknown(b) => format!("unknown status {:?}", char::from(b)),
    }
}

fn xy(f: &FileStatus) -> String {
    f.xy.iter()
        .map(|&b| {
            if b.is_ascii_graphic() {
                char::from(b)
            } else {
                '?'
            }
        })
        .collect()
}

fn entry_path(f: &FileStatus) -> String {
    match &f.original_path {
        Some(orig) => format!("{} -> {}", display_path(orig), display_path(&f.path)),
        None => display_path(&f.path),
    }
}
