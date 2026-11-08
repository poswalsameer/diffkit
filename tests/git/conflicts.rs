//! Unresolved merge conflicts: index stages (base, ours, theirs) read
//! without touching the index, every stage combination Git produces, and
//! the interface's conflict presentation. Every inspection is checked
//! against a byte-for-byte fingerprint of the repository.

use crate::support;

use diffkit::{
    tui::actions::Action,
    types::{
        diff::{ConflictInfo, DiffComparison::*, DiffContent, FileChangeKind},
        repository::{ConflictKind, ConflictStages},
    },
};
use std::path::Path;
use support::{assert_read_only, discover, engine, entry, screen_text, tui_app, Repo, TempDir};

/// A repository mid-merge with one path in each conflict shape:
/// - `both.txt`: modified on both sides (`UU`, all three stages);
/// - `added.txt`: added on both sides with different content (`AA`);
/// - `theirs-deleted.txt`: we modified it, they deleted it (`UD`);
/// - `ours-deleted.txt`: we deleted it, they modified it (`DU`);
/// - `plain.txt`: an ordinary unstaged edit, not a conflict.
fn conflicted(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    for f in [
        "both.txt",
        "theirs-deleted.txt",
        "ours-deleted.txt",
        "plain.txt",
    ] {
        repo.write(f, "base\n");
    }
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("both.txt", "side\n");
    repo.write("added.txt", "side added\n");
    repo.remove("theirs-deleted.txt");
    repo.write("ours-deleted.txt", "side edit\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("both.txt", "main\n");
    repo.write("added.txt", "main added\n");
    repo.write("theirs-deleted.txt", "main edit\n");
    repo.remove("ours-deleted.txt");
    repo.commit_all("main");
    repo.failing_merge("side");
    repo.write("plain.txt", "base\nedited during the merge\n");
    (tmp, repo)
}

/// `git ls-files --unmerged` for one path: (stage number, object id).
fn git_stages(repo: &Repo, path: &str) -> Vec<(u8, String)> {
    repo.git(&["ls-files", "--unmerged", "-z", "--", path])
        .split('\0')
        .filter(|r| !r.is_empty())
        .map(|r| {
            let (meta, _) = r.split_once('\t').unwrap();
            let f: Vec<&str> = meta.split(' ').collect();
            (f[2].parse().unwrap(), f[1].to_string())
        })
        .collect()
}

fn stage_ids(s: &ConflictStages) -> Vec<(u8, String)> {
    [(1, &s.base), (2, &s.ours), (3, &s.theirs)]
        .into_iter()
        .filter_map(|(n, st)| st.as_ref().map(|st| (n, st.oid.to_string())))
        .collect()
}

fn conflict_of(repo: &Repo, path: &str) -> ConflictInfo {
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    let f = entry(&list, path);
    assert_eq!(f.kind, FileChangeKind::Unmerged, "{path}");
    assert!(f.conflicted);
    match e.load_file_diff(&list, f.id).unwrap().content {
        DiffContent::Unmerged(c) => c,
        other => panic!("{path}: expected a conflict, got {other:?}"),
    }
}

#[test]
fn every_conflict_shape_reports_exactly_the_stages_git_recorded() {
    let (_t, repo) = conflicted("conflict-shapes");
    let cases = [
        ("both.txt", ConflictKind::BothModified, [true, true, true]),
        ("added.txt", ConflictKind::BothAdded, [false, true, true]),
        (
            "theirs-deleted.txt",
            ConflictKind::DeletedByThem,
            [true, true, false],
        ),
        (
            "ours-deleted.txt",
            ConflictKind::DeletedByUs,
            [true, false, true],
        ),
    ];
    assert_read_only(&repo.path, || {
        for (path, kind, present) in cases {
            let c = conflict_of(&repo, path);
            assert_eq!(c.kind, Some(kind), "{path}");
            let stages = c.stages.as_ref().expect("stages");
            assert_eq!(
                [
                    stages.base.is_some(),
                    stages.ours.is_some(),
                    stages.theirs.is_some()
                ],
                present,
                "{path}: no stage is invented or dropped"
            );
            assert_eq!(stage_ids(stages), git_stages(&repo, path), "{path}");
            assert_eq!(
                stages.kind(),
                Some(kind),
                "{path}: shape matches Git's code"
            );
        }
    });
}

#[test]
fn working_tree_presence_follows_the_file() {
    let (_t, repo) = conflicted("conflict-worktree");
    // Git writes the file with conflict markers into the working tree.
    assert!(conflict_of(&repo, "both.txt").worktree_present);
    std::fs::remove_file(repo.path.join("both.txt")).unwrap();
    let c = conflict_of(&repo, "both.txt");
    assert!(
        !c.worktree_present,
        "a removed conflicted file is reported as missing"
    );
    assert_eq!(c.kind, Some(ConflictKind::BothModified));
}

#[test]
fn conflicts_are_distinct_from_ordinary_changes() {
    let (_t, repo) = conflicted("conflict-distinct");
    let e = engine(&repo.path);
    assert_read_only(&repo.path, || {
        for c in [Staged, Unstaged] {
            let list = discover(&e, c);
            let conflicted: Vec<_> = list
                .files
                .iter()
                .filter(|f| f.conflicted)
                .map(|f| f.path().to_string_lossy().into_owned())
                .collect();
            assert_eq!(conflicted.len(), 4, "{c:?}: {conflicted:?}");
            assert!(list
                .files
                .iter()
                .filter(|f| f.conflicted)
                .all(|f| f.kind == FileChangeKind::Unmerged));
        }
        let unstaged = discover(&e, Unstaged);
        let plain = entry(&unstaged, "plain.txt");
        assert_eq!(plain.kind, FileChangeKind::Modified);
        assert!(!plain.conflicted);
        let d = e.load_file_diff(&unstaged, plain.id).unwrap();
        assert!(matches!(d.content, DiffContent::Text { .. }));
    });
}

#[test]
fn snapshot_carries_stages_and_inspect_reports_them() {
    let (_t, repo) = conflicted("conflict-inspect");
    let s = repo.snapshot();
    let f = support::find(&s, "added.txt");
    assert_eq!(f.conflict, Some(ConflictKind::BothAdded));
    assert!(f.stages.as_ref().unwrap().base.is_none());
    let out = assert_read_only(&repo.path, || support::diffkit(&repo.path, &["inspect"]));
    assert!(out.status.success());
    let text = support::stdout(&out);
    assert!(
        text.contains("A merge is in progress (MERGE_HEAD exists)."),
        "{text}"
    );
    assert!(
        text.contains("\nCONFLICT STAGES (index stages Git recorded)\n"),
        "{text}"
    );
    let both = git_stages(&repo, "both.txt");
    assert!(
        text.contains(&format!(
            "  both.txt  base {}  ours {}  theirs {}\n",
            &both[0].1[..12],
            &both[1].1[..12],
            &both[2].1[..12]
        )),
        "{text}"
    );
    assert!(text.contains("  added.txt  base (none)  ours "), "{text}");
}

#[test]
fn interface_explains_each_conflict_and_never_resolves_it() {
    let (_t, repo) = conflicted("conflict-tui");
    assert_read_only(&repo.path, || {
        let mut app = tui_app(&repo.path, 140, 40);
        app.dispatch(Action::SetComparison(Unstaged));
        let text = screen_text(&app);
        assert!(text.contains("MERGING"), "{text}");
        assert!(text.contains("⚠ 4 conflicts"), "{text}");
        let mut seen = 0;
        for _ in 0..app.mode_state(Unstaged).files().len() {
            let f = app.selected_file().unwrap().clone();
            let text = screen_text(&app);
            if f.path() == Path::new("added.txt") {
                assert!(
                    text.contains("⚠ CONFLICT — this path is unmerged"),
                    "{text}"
                );
                assert!(text.contains("Git reports: both added."), "{text}");
                assert!(text.contains("base    (no version on this side)"), "{text}");
                assert!(text.contains("ours    "), "{text}");
                assert!(text.contains("Diffkit never resolves conflicts"), "{text}");
                seen += 1;
            }
            if f.path() == Path::new("ours-deleted.txt") {
                assert!(text.contains("Git reports: deleted by us."), "{text}");
                assert!(text.contains("ours    (no version on this side)"), "{text}");
                seen += 1;
            }
            app.dispatch(Action::SelectNextFile);
        }
        assert_eq!(seen, 2);
    });
}
