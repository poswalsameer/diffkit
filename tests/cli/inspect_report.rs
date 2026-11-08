//! The `diffkit inspect` report, rendered from constructed snapshots.

use diffkit::{cli::commands::inspect::render, types::repository::*};
use std::path::PathBuf;

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
        stages: None,
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
        operations: Vec::new(),
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
    assert!(out
        .contains("\nUNSTAGED (working tree differs from index)\n  MM  src/main.rs  (modified)\n"));
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
    assert!(out.contains("State:\n  Clean: no staged, unstaged, untracked, or conflicted files\n"));
    assert!(out.contains("FILES\n\n  (none)\n"));
    assert!(out.contains("Operation in progress:\n  None\n"));
}

#[test]
fn operations_and_conflict_stages_are_reported() {
    let mut c = file("f", b"AA", Change::Unmerged, Change::Unmerged);
    c.conflict = ConflictKind::from_xy(*b"AA");
    let stage = ConflictStage {
        mode: 0o100644,
        oid: ObjectId::parse(&[b'c'; 40]).unwrap(),
    };
    c.stages = Some(ConflictStages {
        base: None,
        ours: Some(stage.clone()),
        theirs: Some(stage),
    });
    let mut snap = snapshot(
        RepositoryStatus {
            files: vec![c],
            ignored: vec![],
        },
        UpstreamInfo::NotConfigured,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream),
    );
    snap.operations = vec![Operation::Merge, Operation::Bisect { start: None }];
    let out = render(&snap);
    assert!(out.contains(
        "Operation in progress:\n  A merge is in progress (MERGE_HEAD exists).\n  A bisect is in progress.\n"
    ));
    assert!(out.contains("  AA  f  (both added)\n"));
    assert!(out.contains(
        "\nCONFLICT STAGES (index stages Git recorded)\n  f  base (none)  ours cccccccccccc  theirs cccccccccccc\n"
    ));
}
