//! Formats a [`RepositorySnapshot`] as the plain-text inspection report.
//!
//! All values come from the snapshot. Paths are displayed unambiguously:
//! names with control characters, quotes, backslashes, surrounding
//! whitespace, ` -> `, or bytes that are not UTF-8 are quoted and escaped
//! the way Git quotes paths; the snapshot itself keeps the raw path.

use diffkit::git::models::{
    BranchInfo, Change, CommitTrackingInfo, FileStatus, HeadState, RepositoryKind,
    RepositorySnapshot, RepositoryStatus, TrackingRelation, TrackingUnavailable, UpstreamInfo,
};
use std::fmt::Write;

const RULE: &str = "------------------------------------------------------------";

pub fn render(s: &RepositorySnapshot) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{RULE}\nDIFFKIT - REPOSITORY INSPECTOR\n{RULE}");
    repository(&mut out, s);
    branch(&mut out, &s.branch);
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

pub use diffkit::display::display_path;

#[cfg(test)]
mod tests {
    use super::*;
    use diffkit::git::models::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn plain_paths_are_unquoted() {
        assert_eq!(display_path(Path::new("src/main.rs")), "src/main.rs");
        assert_eq!(
            display_path(Path::new("dir/a file \u{65e5}.txt")),
            "dir/a file \u{65e5}.txt"
        );
    }

    #[test]
    fn ambiguous_paths_are_quoted_and_escaped() {
        assert_eq!(display_path(Path::new("a\nb\tc")), "\"a\\nb\\tc\"");
        assert_eq!(display_path(Path::new("q\"b\\s")), "\"q\\\"b\\\\s\"");
        assert_eq!(display_path(Path::new(" lead")), "\" lead\"");
        assert_eq!(display_path(Path::new("trail ")), "\"trail \"");
        assert_eq!(display_path(Path::new("a -> b")), "\"a -> b\"");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_bytes_are_shown_as_hex_escapes() {
        use std::os::unix::ffi::OsStrExt;
        let p = Path::new(std::ffi::OsStr::from_bytes(b"raw-\xff.txt"));
        assert_eq!(display_path(p), "\"raw-\\xff.txt\"");
    }

    fn file(path: &str, xy: &[u8; 2], index: Change, worktree: Change) -> FileStatus {
        FileStatus {
            path: PathBuf::from(path),
            original_path: None,
            index,
            worktree,
            xy: *xy,
            conflict: None,
            similarity: None,
            file_kind: Some(FileKind::Regular),
            submodule: None,
        }
    }

    fn snapshot(
        status: RepositoryStatus,
        upstream: UpstreamInfo,
        t: CommitTrackingInfo,
    ) -> RepositorySnapshot {
        RepositorySnapshot {
            repository: RepositoryInfo {
                root: "/r".into(),
                git_dir: "/r/.git".into(),
                common_git_dir: "/r/.git".into(),
                current_directory: "/r".into(),
                kind: RepositoryKind::Standard,
                is_inside_work_tree: true,
                shallow: false,
                object_format: ObjectFormat::Sha1,
            },
            branch: BranchInfo {
                head: HeadState::Branch("main".into()),
                head_commit: ObjectId::parse(&[b'a'; 40]),
                empty_repository: None,
            },
            upstream,
            commit_tracking: t,
            status,
        }
    }

    #[test]
    fn partially_staged_file_appears_with_both_dimensions() {
        let status = RepositoryStatus {
            files: vec![file(
                "src/main.rs",
                b"MM",
                Change::Modified,
                Change::Modified,
            )],
            ignored: vec![],
        };
        let out = render(&snapshot(
            status,
            UpstreamInfo::NotConfigured,
            CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream),
        ));
        assert!(out.contains("\nPARTIALLY STAGED (both staged and unstaged changes)\n  MM  src/main.rs  (staged modified, unstaged modified)\n"));
        assert!(out.contains("\nSTAGED (index differs from HEAD)\n  MM  src/main.rs  (modified)\n"));
        assert!(out.contains(
            "\nUNSTAGED (working tree differs from index)\n  MM  src/main.rs  (modified)\n"
        ));
        assert!(out.contains("Partially staged:\n  1 file\n"));
        assert!(out.contains("No upstream configured for branch main"));
        assert!(out.contains("Unavailable: no upstream is configured"));
        assert!(!out.contains("Ahead by"));
        assert!(!out.contains("Information source"));
    }

    #[test]
    fn tracking_counts_carry_the_staleness_disclaimer() {
        let up = UpstreamInfo::Available {
            name: "origin/main".into(),
            full_ref: Some("refs/remotes/origin/main".into()),
            commit: ObjectId::parse(&[b'b'; 40]).unwrap(),
        };
        let out = render(&snapshot(
            RepositoryStatus::default(),
            up,
            CommitTrackingInfo::Compared {
                ahead: 2,
                behind: 1,
            },
        ));
        assert!(out.contains("Upstream:\n  origin/main (refs/remotes/origin/main)\n"));
        assert!(out.contains(
            "Tracking status:\n  Diverged from the locally known upstream\n  Ahead by 2 commits\n  Behind by 1 commit\n"
        ));
        assert!(out.contains("Remote state has not been verified by fetching"));
        assert!(
            out.contains("State:\n  Clean: no staged, unstaged, untracked, or conflicted files\n")
        );
        assert!(out.contains("FILES\n\n  (none)\n"));
    }
}
