//! Reconciliation against real repositories: list reuse is only taken when
//! provably safe, unchanged state is detected, and ignored directories are
//! found through Git.

mod common;

use common::{Repo, TempDir};
use diffkit::{
    diff::{DiffComparison, DiffOptions},
    git::commands::thread_invocations,
    sync::{reconcile, ChangeSet},
};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit_all("base");
    (tmp, repo)
}

fn files(r: &reconcile::Reconciled, c: DiffComparison) -> Vec<String> {
    let i = DiffComparison::ALL.iter().position(|&m| m == c).unwrap();
    r.lists[i]
        .as_ref()
        .unwrap()
        .files
        .iter()
        .map(|f| f.path().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn refs_only_change_reuses_every_list() {
    let (_t, repo) = repo("rec-refs");
    repo.write("a.txt", "changed\n");
    common::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    repo.git(&["branch", "side"]);
    let before = thread_invocations();
    let r = reconcile::reconcile(&repo.path, opts, ChangeSet::REFS, Some(&prev)).unwrap();
    assert_eq!(r.reused, 4);
    assert_eq!(
        thread_invocations() - before,
        2,
        "snapshot only: rev-parse + status"
    );
    assert!(r.same_state(&prev));
}

#[test]
fn worktree_change_reuses_only_the_staged_list() {
    let (_t, repo) = repo("rec-worktree");
    repo.write("b.txt", "staged\n");
    repo.git(&["add", "b.txt"]);
    common::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    repo.write("a.txt", "edited\n");
    let r = reconcile::reconcile(&repo.path, opts, ChangeSet::WORKTREE, Some(&prev)).unwrap();
    assert_eq!(r.reused, 1);
    assert_eq!(files(&r, DiffComparison::Staged), ["b.txt"]);
    assert_eq!(files(&r, DiffComparison::Unstaged), ["a.txt"]);
    assert!(!r.same_state(&prev));
}

#[test]
fn moved_head_is_never_reused_whatever_the_trigger_says() {
    let (_t, repo) = repo("rec-head");
    common::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    repo.write("a.txt", "committed\n");
    repo.commit_all("move head");
    repo.write("b.txt", "dirty\n");
    // Even if a commit were (wrongly) reported as a refs-only change, the
    // changed HEAD makes every list rediscovered.
    let r = reconcile::reconcile(&repo.path, opts, ChangeSet::REFS, Some(&prev)).unwrap();
    assert_eq!(r.reused, 0);
    assert_eq!(files(&r, DiffComparison::Unstaged), ["b.txt"]);
}

#[test]
fn index_and_manual_triggers_rediscover_everything() {
    let (_t, repo) = repo("rec-index");
    common::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    repo.write("a.txt", "x\n");
    repo.git(&["add", "a.txt"]);
    for trigger in [
        ChangeSet::INDEX,
        ChangeSet::MANUAL,
        ChangeSet::OVERFLOW,
        ChangeSet::INDEX | ChangeSet::REFS,
    ] {
        let r = reconcile::reconcile(&repo.path, opts, trigger, Some(&prev)).unwrap();
        assert_eq!(r.reused, 0, "{trigger:?}");
        assert_eq!(files(&r, DiffComparison::Staged), ["a.txt"]);
    }
}

#[test]
fn ignored_directories_come_from_git() {
    let (_t, repo) = repo("rec-ignored");
    repo.write(".gitignore", "build/\nnode_modules/\n*.log\n:(attr)odd/\n");
    repo.write("kept/target/tracked.rs", "tracked\n");
    repo.commit_all("rules");
    repo.write("build/out.o", "x");
    repo.write("web/node_modules/pkg/index.js", "x");
    repo.write("debug.log", "x");
    repo.write(":(attr)odd/y", "pathspec-magic-looking directory name");
    repo.write("kept/target/new.rs", "untracked, not ignored");
    common::isolate();
    let mut dirs = reconcile::ignored_directories(&repo.path).unwrap();
    dirs.sort();
    // `web/` holds only ignored content today but is not ignored itself:
    // a new file in it must still be noticed, so it is not excluded.
    assert_eq!(
        dirs,
        vec![
            repo.path.join(":(attr)odd"),
            repo.path.join("build"),
            repo.path.join("web/node_modules")
        ]
    );
    // A tracked `target` directory is never excluded.
    assert!(!dirs.iter().any(|d| d.ends_with("target")));
}
