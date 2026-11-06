//! End to end: the real filesystem watcher and Git reconciler on temporary
//! repositories, driven by external Git and filesystem operations. Each test
//! waits (up to 15 s) for the expected state rather than sleeping fixed
//! amounts, so timing differences cannot make them flaky.

mod common;

use common::{Repo, TempDir};
use diffkit::{
    diff::{DiffComparison, DiffOptions, FileChangeKind},
    git::models::CommitTrackingInfo,
    sync::{
        reconcile, GitReconciler, Reconciled, SyncConfig, SyncDiagnostics, SyncEvent, SyncService,
        SyncUpdate,
    },
};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(15);

struct Session {
    service: Option<SyncService>,
    rx: Receiver<SyncEvent>,
    state: Reconciled,
    diag: SyncDiagnostics,
    updates: Vec<String>,
}

impl Session {
    fn start(path: &Path) -> Self {
        common::isolate();
        let opts = DiffOptions::default();
        let state =
            reconcile::reconcile(path, opts, diffkit::sync::ChangeSet::STARTUP, None).unwrap();
        let (tx, rx) = mpsc::channel();
        let config = SyncConfig {
            quiet: Duration::from_millis(100),
            max_delay: Duration::from_millis(1000),
            ..SyncConfig::default()
        };
        let service = SyncService::start(
            config,
            GitReconciler::new(path.to_path_buf(), opts, state.clone()),
            move |e| {
                let _ = tx.send(e);
            },
        );
        let mut s = Self {
            service: Some(service),
            rx,
            state,
            diag: SyncDiagnostics::default(),
            updates: Vec::new(),
        };
        // The startup reconciliation runs once the watcher is active.
        s.wait_for("startup reconciliation", |_, d| d.reconciles_completed >= 1);
        s
    }

    fn pump(&mut self, timeout: Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(e) => {
                self.diag = e.diagnostics;
                self.updates
                    .push(format!("{:?}", std::mem::discriminant(&e.update)));
                match e.update {
                    SyncUpdate::Reconciled(r) => self.state = *r,
                    SyncUpdate::WatcherUnavailable(m) => {
                        self.updates.push(format!("unavailable: {m}"))
                    }
                    SyncUpdate::Failed(m) => self.updates.push(format!("failed: {m}")),
                    _ => {}
                }
                true
            }
            Err(_) => false,
        }
    }

    fn wait_for(&mut self, what: &str, pred: impl Fn(&Reconciled, &SyncDiagnostics) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !pred(&self.state, &self.diag) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!(
                    "timed out waiting for {what}; diagnostics {:#?}; updates {:?}",
                    self.diag, self.updates
                );
            }
            self.pump(left);
        }
    }

    /// Lets the worker go quiet, collecting anything it publishes.
    fn settle(&mut self, quiet: Duration) {
        while self.pump(quiet) {}
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(s) = self.service.take() {
            assert!(s.shutdown(Duration::from_secs(5)), "worker stops promptly");
        }
    }
}

fn paths(r: &Reconciled, c: DiffComparison) -> Vec<String> {
    let i = DiffComparison::ALL.iter().position(|&m| m == c).unwrap();
    match &r.lists[i] {
        Ok(l) => l
            .files
            .iter()
            .map(|f| f.path().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn kind_of(r: &Reconciled, c: DiffComparison, path: &str) -> Option<FileChangeKind> {
    let i = DiffComparison::ALL.iter().position(|&m| m == c).unwrap();
    r.lists[i]
        .as_ref()
        .ok()?
        .files
        .iter()
        .find(|f| f.path() == Path::new(path))
        .map(|f| f.kind)
}

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, &format!("{f}\n"));
    }
    repo.commit_all("base");
    (tmp, repo)
}

use DiffComparison::*;

#[test]
fn editing_a_tracked_file_updates_unstaged_and_combined() {
    let (_t, repo) = repo("watch-edit");
    let mut s = Session::start(&repo.path);
    repo.write("a.txt", "edited in an editor\n");
    s.wait_for("a.txt unstaged", |r, _| {
        paths(r, Unstaged) == ["a.txt"] && paths(r, Combined) == ["a.txt"]
    });
    assert!(paths(&s.state, Staged).is_empty());
}

#[test]
fn staging_and_unstaging_in_another_terminal() {
    let (_t, repo) = repo("watch-stage");
    let mut s = Session::start(&repo.path);
    repo.write("b.txt", "to be staged\n");
    s.wait_for("unstaged", |r, _| paths(r, Unstaged) == ["b.txt"]);
    repo.git(&["add", "b.txt"]);
    s.wait_for("staged", |r, _| {
        paths(r, Staged) == ["b.txt"] && paths(r, Unstaged).is_empty()
    });
    repo.git(&["reset", "-q", "--", "b.txt"]);
    s.wait_for("unstaged again", |r, _| {
        paths(r, Staged).is_empty() && paths(r, Unstaged) == ["b.txt"]
    });
}

#[test]
fn untracked_files_appear_and_disappear() {
    let (_t, repo) = repo("watch-untracked");
    let mut s = Session::start(&repo.path);
    repo.write("notes/new idea.md", "x\n");
    s.wait_for("untracked", |r, _| {
        paths(r, Untracked) == ["notes/new idea.md"]
    });
    std::fs::remove_dir_all(repo.path.join("notes")).unwrap();
    s.wait_for("gone", |r, _| paths(r, Untracked).is_empty());
}

#[test]
fn deletion_and_rename() {
    let (_t, repo) = repo("watch-delete-rename");
    let mut s = Session::start(&repo.path);
    std::fs::remove_file(repo.path.join("c.txt")).unwrap();
    s.wait_for("deleted", |r, _| {
        kind_of(r, Unstaged, "c.txt") == Some(FileChangeKind::Deleted)
    });
    repo.git(&["mv", "a.txt", "renamed.txt"]);
    s.wait_for("renamed", |r, _| {
        kind_of(r, Staged, "renamed.txt") == Some(FileChangeKind::Renamed)
    });
}

#[test]
fn external_commit_and_branch_switch() {
    let (_t, repo) = repo("watch-commit-switch");
    let mut s = Session::start(&repo.path);
    repo.write("a.txt", "committed change\n");
    repo.git(&["add", "a.txt"]);
    s.wait_for("staged", |r, _| paths(r, Staged) == ["a.txt"]);
    let head = repo.commit_all("external commit");
    s.wait_for("committed", |r, _| {
        r.snapshot.branch.head_commit.as_ref().map(|c| c.as_str()) == Some(head.as_str())
            && paths(r, Staged).is_empty()
    });
    repo.git(&["switch", "-q", "-c", "feature/x"]);
    s.wait_for("branch switched", |r, _| {
        r.snapshot.branch.name() == Some("feature/x")
    });
    repo.git(&["switch", "-q", "main"]);
    s.wait_for("back on main", |r, _| {
        r.snapshot.branch.name() == Some("main")
    });
}

#[test]
fn conflict_introduced_and_resolved_externally() {
    let (_t, repo) = repo("watch-conflict");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("a.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("a.txt", "main\n");
    repo.commit_all("main");
    let mut s = Session::start(&repo.path);
    assert!(!repo.git_output(&["merge", "side"]).status.success());
    s.wait_for("conflict", |r, _| {
        kind_of(r, Unstaged, "a.txt") == Some(FileChangeKind::Unmerged)
    });
    assert!(s.state.snapshot.status.conflicted_count() == 1);
    repo.write("a.txt", "resolved\n");
    repo.git(&["add", "a.txt"]);
    s.wait_for("resolved", |r, _| {
        kind_of(r, Staged, "a.txt") == Some(FileChangeKind::Modified)
            && r.snapshot.status.conflicted_count() == 0
    });
}

#[test]
fn bursts_are_debounced_into_few_reconciliations() {
    let (_t, repo) = repo("watch-burst");
    let mut s = Session::start(&repo.path);
    s.settle(Duration::from_millis(300));
    let before = s.diag;
    for i in 0..100 {
        let f = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"][i % 5];
        repo.write(f, &format!("save {i}\n"));
    }
    s.wait_for("final content", |r, _| {
        paths(r, Unstaged).len() == 3 && paths(r, Untracked).len() == 2
    });
    s.settle(Duration::from_millis(600));
    let runs = s.diag.reconcile_requests - before.reconcile_requests;
    let events = s.diag.fs_events - before.fs_events;
    assert!(runs <= 3, "{runs} reconciliations for {events} events");
    assert!(events >= runs, "{events} events");
}

#[test]
fn ignored_directories_and_own_git_commands_cause_no_reconciliation() {
    let (_t, repo) = repo("watch-ignored");
    repo.write(".gitignore", "build/\n");
    repo.commit_all("ignore build");
    repo.write("build/seed.o", "x");
    let mut s = Session::start(&repo.path);
    s.settle(Duration::from_millis(500));
    let before = s.diag.reconcile_requests;
    for i in 0..50 {
        repo.write(format!("build/out{i}.o"), "artifact");
    }
    s.settle(Duration::from_millis(800));
    assert_eq!(
        s.diag.reconcile_requests, before,
        "ignored build output and idle Git reads: no runs"
    );
    repo.write("b.txt", "real change\n");
    s.wait_for("real change still seen", |r, _| {
        paths(r, Unstaged) == ["b.txt"]
    });
}

#[test]
fn removing_the_repository_degrades_without_crashing() {
    let (t, repo) = repo("watch-removed");
    let mut s = Session::start(&repo.path);
    std::fs::rename(&repo.path, t.join("moved-away")).unwrap();
    s.wait_for("failure reported", |_, d| d.reconcile_failures >= 1);
}

#[test]
fn linked_worktree_and_shared_references() {
    let (t, main) = repo("watch-worktree");
    let wt_path = t.join("linked");
    main.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "topic",
        wt_path.to_str().unwrap(),
    ]);
    let wt = Repo {
        path: common::real(&wt_path),
    };
    wt.git(&["branch", "-q", "--set-upstream-to=main"]);
    let mut s = Session::start(&wt.path);
    // Working tree of the linked worktree.
    wt.write("a.txt", "edited in the worktree\n");
    s.wait_for("worktree edit", |r, _| paths(r, Unstaged) == ["a.txt"]);
    // HEAD and index in the per-worktree Git directory.
    wt.git(&["add", "a.txt"]);
    wt.git(&["commit", "-q", "-m", "worktree commit"]);
    let wt_head = wt.head();
    s.wait_for("worktree commit", |r, _| {
        r.snapshot.branch.head_commit.as_ref().map(|c| c.as_str()) == Some(wt_head.as_str())
            && paths(r, Staged).is_empty()
    });
    // A shared ref changed from the main worktree: topic's upstream (main)
    // moves, so this worktree is now behind.
    main.write("b.txt", "main moved\n");
    main.commit_all("main commit");
    s.wait_for("shared ref change", |r, _| {
        matches!(
            r.snapshot.commit_tracking,
            CommitTrackingInfo::Compared { behind: 1, .. }
        )
    });
}

fn git_fingerprint(
    dir: &Path,
) -> Vec<(std::path::PathBuf, Vec<u8>, Option<std::time::SystemTime>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p);
            } else {
                out.push((
                    p.clone(),
                    std::fs::read(&p).unwrap_or_default(),
                    m.modified().ok(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn synchronization_never_writes_git_metadata() {
    let (_t, repo) = repo("watch-readonly");
    let git_dir = repo.path.join(".git");
    let before = git_fingerprint(&git_dir);
    let mut s = Session::start(&repo.path);
    // A stat-only change (same content, new mtime) is what tempts Git to
    // refresh and rewrite the index.
    let f = std::fs::File::options()
        .write(true)
        .open(repo.path.join("a.txt"))
        .unwrap();
    f.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    repo.write("b.txt", "edited\n");
    repo.write("new.txt", "untracked\n");
    s.wait_for("edits seen", |r, _| {
        paths(r, Unstaged) == ["b.txt"] && paths(r, Untracked) == ["new.txt"]
    });
    s.service.as_ref().unwrap().request_refresh();
    s.wait_for("manual refresh", |_, d| d.reconcile_requests >= 3);
    s.settle(Duration::from_millis(400));
    assert_eq!(
        git_fingerprint(&git_dir),
        before,
        ".git is byte-for-byte unchanged"
    );
}
