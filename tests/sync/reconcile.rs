//! Reconciliation against real repositories: list reuse is only taken when
//! provably safe, unchanged state is detected, and ignored directories are
//! found through Git.

use crate::support;

use diffkit::{
    diff::engine::DiffEngine,
    git::commands::thread_invocations,
    sync::{events::ChangeSet, reconcile},
    types::{
        diff::{DiffComparison, DiffOptions},
        repository::CommitTrackingInfo,
    },
};
use support::{Repo, TempDir};

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
    support::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    repo.git(&["branch", "side"]);
    let before = thread_invocations();
    let r = reconcile::reconcile(
        &repo.path,
        opts,
        ChangeSet::REFS | ChangeSet::CONFIG,
        Some(&prev),
    )
    .unwrap();
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
    support::isolate();
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
    support::isolate();
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
    support::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    repo.write("a.txt", "x\n");
    repo.git(&["add", "a.txt"]);
    // Any trigger outside the reuse rules (working tree, ignore rules,
    // refs, config) rediscovers every list, even combined with refs.
    for trigger in [
        ChangeSet::INDEX,
        ChangeSet::HEAD,
        ChangeSet::MERGE_STATE,
        ChangeSet::REBASE_STATE,
        ChangeSet::STRUCTURE,
        ChangeSet::OVERFLOW,
        ChangeSet::MANUAL,
        ChangeSet::STARTUP,
    ]
    .map(|t| [t, t | ChangeSet::REFS])
    .into_iter()
    .flatten()
    {
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
    support::isolate();
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

/// Gives `rel` an old modification time without changing its content, so
/// its stat data no longer matches the index.
fn touch_old(repo: &Repo, rel: &str) {
    let f = std::fs::File::options()
        .write(true)
        .open(repo.path.join(rel))
        .unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000))
        .unwrap();
}

/// Lists limited to the paths status reported equal whole-tree discovery.
fn assert_reported_lists_match(repo: &Repo) -> Vec<diffkit::types::diff::DiffFileList> {
    let snapshot = repo.snapshot();
    let engine = DiffEngine::from_snapshot(&snapshot, DiffOptions::default());
    DiffComparison::ALL
        .iter()
        .map(|&c| {
            let whole = engine.discover(c).unwrap();
            let reported = engine.discover_reported(c, &snapshot.status).unwrap();
            assert_eq!(reported.files, whole.files, "{c:?}");
            assert_eq!(reported.base, whole.base, "{c:?}");
            reported
        })
        .collect()
}

#[test]
fn reported_lists_match_whole_tree_discovery() {
    let (_t, repo) = repo("rec-reported");
    repo.write("c.txt", "one\ntwo\nthree\nfour\n");
    repo.write("d.txt", "d\n");
    repo.write("sub/e.txt", "e\n");
    repo.commit_all("more");
    support::isolate();
    // A staged rename edited again in the working tree, a staged edit, a
    // working-tree deletion, an intent-to-add file, a file removed from
    // the index only, untracked files including a nested repository, and
    // a file whose stat data alone changed.
    repo.git(&["mv", "c.txt", "renamed.txt"]);
    repo.write("renamed.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.write("a.txt", "staged edit\n");
    repo.git(&["add", "a.txt"]);
    repo.remove("d.txt");
    repo.write("ita.txt", "intent\n");
    repo.git(&["add", "-N", "ita.txt"]);
    repo.git(&["rm", "-q", "--cached", "sub/e.txt"]);
    repo.write("u1.txt", "u\n");
    repo.write("dir/u2.txt", "u\n");
    Repo::init(repo.path.join("nested"));
    touch_old(&repo, "b.txt");
    let lists = assert_reported_lists_match(&repo);
    let untracked: Vec<_> = lists[3].files.iter().map(|f| f.path().to_owned()).collect();
    assert!(
        untracked.contains(&std::path::PathBuf::from("nested")),
        "{untracked:?}"
    );
    // The whole tree also examined b.txt and found its content unchanged;
    // the reported paths never included it.
    let engine = support::engine(&repo.path);
    assert_eq!(
        engine
            .discover(DiffComparison::Unstaged)
            .unwrap()
            .stat_only_entries,
        1
    );
    assert_eq!(lists[1].stat_only_entries, 0);
}

#[test]
fn reported_lists_match_during_a_conflict() {
    let (_t, repo) = repo("rec-reported-conflict");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("a.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("a.txt", "main\n");
    repo.write("b.txt", "main too\n");
    repo.commit_all("main");
    support::isolate();
    repo.failing_merge("side");
    let lists = assert_reported_lists_match(&repo);
    assert!(lists[1].files.iter().any(|f| f.conflicted));
}

#[test]
fn many_reported_paths_fall_back_to_the_whole_tree() {
    let (_t, repo) = repo("rec-reported-many");
    let names: Vec<String> = (0..200)
        .map(|i| format!("{}-{i:03}.txt", "long-name".repeat(12)))
        .collect();
    for n in &names {
        repo.write(n, "base\n");
    }
    repo.commit_all("many");
    support::isolate();
    for n in &names {
        repo.write(n, "edited\n");
    }
    touch_old(&repo, "b.txt");
    let bytes: usize = names.iter().map(|n| n.len() + 1).sum();
    assert!(bytes > diffkit::diff::engine::MAX_PATHSPEC_BYTES);
    let lists = assert_reported_lists_match(&repo);
    assert_eq!(lists[1].len(), 200);
    // Only a whole-tree comparison examines (and counts) b.txt.
    assert_eq!(lists[1].stat_only_entries, 1);
}

#[test]
fn clean_tree_runs_no_list_commands() {
    let (_t, repo) = repo("rec-clean");
    support::isolate();
    let before = thread_invocations();
    let r =
        reconcile::reconcile(&repo.path, DiffOptions::default(), ChangeSet::STARTUP, None).unwrap();
    assert_eq!(thread_invocations() - before, 2, "rev-parse + status only");
    assert!(r.lists.iter().all(|l| l.as_ref().unwrap().is_empty()));
}

#[test]
fn worktree_change_reuses_repository_and_upstream() {
    let r = support::Remote::new("rec-upstream");
    let repo = &r.work;
    support::isolate();
    let opts = DiffOptions::default();
    let prev = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    assert_eq!(
        prev.snapshot.commit_tracking,
        CommitTrackingInfo::Compared {
            ahead: 0,
            behind: 0
        }
    );
    repo.write("file.txt", "edited\n");
    let before = thread_invocations();
    let next = reconcile::reconcile(&repo.path, opts, ChangeSet::WORKTREE, Some(&prev)).unwrap();
    assert_eq!(
        thread_invocations() - before,
        3,
        "status, unstaged, combined: no rev-parse, upstream lookup, or staged list"
    );
    assert_eq!(next.snapshot.upstream, prev.snapshot.upstream);
    assert_eq!(next.snapshot.commit_tracking, prev.snapshot.commit_tracking);
    assert_eq!(files(&next, DiffComparison::Unstaged), ["file.txt"]);

    // A commit reported (wrongly) as a working-tree change: HEAD no longer
    // matches, so the upstream and counts are computed again.
    repo.commit_all("ahead");
    let moved = reconcile::reconcile(&repo.path, opts, ChangeSet::WORKTREE, Some(&next)).unwrap();
    assert_eq!(
        moved.snapshot.commit_tracking,
        CommitTrackingInfo::Compared {
            ahead: 1,
            behind: 0
        }
    );
    assert!(files(&moved, DiffComparison::Unstaged).is_empty());
}
