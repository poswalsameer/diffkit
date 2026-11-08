//! Repository state types: status classification, conflict kinds and
//! stages, file modes, object IDs, and operation labels.

use diffkit::types::repository::*;
use std::path::PathBuf;

fn entry(xy: &[u8; 2]) -> FileStatus {
    let side = |b| match b {
        b'.' => Change::Unmodified,
        b'M' => Change::Modified,
        b'A' => Change::Added,
        b'D' => Change::Deleted,
        b'R' => Change::Renamed,
        b'C' => Change::Copied,
        b'T' => Change::TypeChanged,
        other => Change::Unknown(other),
    };
    FileStatus {
        path: PathBuf::from("f"),
        original_path: None,
        index: side(xy[0]),
        worktree: side(xy[1]),
        xy: *xy,
        conflict: None,
        similarity: None,
        file_kind: Some(FileKind::Regular),
        submodule: None,
        stages: None,
    }
}

#[test]
fn staged_and_unstaged_are_independent_dimensions() {
    let cases: &[(&[u8; 2], bool, bool)] = &[
        (b"M.", true, false),
        (b".M", false, true),
        (b"MM", true, true),
        (b"A.", true, false),
        (b"AM", true, true),
        (b"AD", true, true),
        (b"D.", true, false),
        (b".D", false, true),
        (b"R.", true, false),
        (b"RM", true, true),
        (b".R", false, true),
        (b"C.", true, false),
        (b"T.", true, false),
        (b".T", false, true),
        (b".A", false, true),
    ];
    for (xy, staged, unstaged) in cases {
        let e = entry(xy);
        let label = String::from_utf8_lossy(&xy[..]).into_owned();
        assert_eq!(e.is_staged(), *staged, "{label} staged");
        assert_eq!(e.is_unstaged(), *unstaged, "{label} unstaged");
        assert_eq!(e.is_partially_staged(), *staged && *unstaged, "{label}");
    }
}

#[test]
fn conflicts_and_untracked_are_neither_staged_nor_unstaged() {
    let mut c = entry(b"UU");
    c.index = Change::Unmerged;
    c.worktree = Change::Unmerged;
    c.conflict = ConflictKind::from_xy(*b"UU");
    assert!(c.is_conflicted() && !c.is_staged() && !c.is_unstaged());
    let mut u = entry(b"??");
    u.index = Change::Untracked;
    u.worktree = Change::Untracked;
    assert!(u.is_untracked() && !u.is_staged() && !u.is_unstaged());
}

#[test]
fn status_counts() {
    let s = RepositoryStatus {
        files: vec![entry(b"MM"), entry(b".D"), entry(b"R."), entry(b"C.")],
        ignored: vec![],
    };
    assert_eq!(s.staged_count(), 3);
    assert_eq!(s.unstaged_count(), 2);
    assert_eq!(s.partially_staged_count(), 1);
    assert_eq!(s.deleted_count(), 1);
    assert_eq!(s.renamed_count(), 1);
    assert_eq!(s.copied_count(), 1);
    assert!(!s.is_clean());
    assert!(RepositoryStatus::default().is_clean());
}

#[test]
fn tracking_relation_and_unavailable_counts() {
    use TrackingRelation::*;
    let c = |ahead, behind| CommitTrackingInfo::Compared { ahead, behind };
    assert_eq!(c(0, 0).relation(), Some(UpToDate));
    assert_eq!(c(3, 0).relation(), Some(Ahead));
    assert_eq!(c(0, 2).relation(), Some(Behind));
    assert_eq!(c(2, 3).relation(), Some(Diverged));
    let none = CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream);
    assert_eq!(
        (none.ahead(), none.behind(), none.relation()),
        (None, None, None)
    );
    assert!(!none.comparison_available());
}

#[test]
fn object_id_validation() {
    assert!(ObjectId::parse(&[b'a'; 40]).is_some());
    assert!(ObjectId::parse(&[b'0'; 64]).is_some());
    assert!(ObjectId::parse(&[b'a'; 39]).is_none());
    assert!(ObjectId::parse(&[b'G'; 40]).is_none());
    assert!(ObjectId::parse(b"(initial)").is_none());
}

#[test]
fn conflict_codes_and_modes() {
    for xy in [b"DD", b"AU", b"UD", b"UA", b"DU", b"AA", b"UU"] {
        assert!(ConflictKind::from_xy(*xy).is_some());
    }
    assert_eq!(ConflictKind::from_xy(*b"MM"), None);
    assert_eq!(FileKind::from_mode(0o120000), Some(FileKind::Symlink));
    assert_eq!(FileKind::from_mode(0), None);
}

#[test]
fn conflict_kind_follows_the_stages_present() {
    let stage = || {
        Some(ConflictStage {
            mode: 0o100644,
            oid: ObjectId::parse(&[b'a'; 40]).unwrap(),
        })
    };
    let s = |b: bool, o: bool, t: bool| ConflictStages {
        base: b.then(stage).flatten(),
        ours: o.then(stage).flatten(),
        theirs: t.then(stage).flatten(),
    };
    let cases = [
        (s(true, false, false), b"DD"),
        (s(false, true, false), b"AU"),
        (s(true, true, false), b"UD"),
        (s(false, false, true), b"UA"),
        (s(true, false, true), b"DU"),
        (s(false, true, true), b"AA"),
        (s(true, true, true), b"UU"),
    ];
    for (stages, xy) in cases {
        assert_eq!(stages.kind(), ConflictKind::from_xy(*xy), "{xy:?}");
    }
    assert_eq!(s(false, false, false).kind(), None);
}
