//! Repository states: operations in progress (merge, rebase with each
//! backend, cherry-pick, revert, bisect), detached and unborn HEAD,
//! per-worktree state in linked worktrees, and missing or unreadable Git
//! metadata. All read-only, all against real repositories.

use crate::support;

use diffkit::{
    sync::{
        events::ChangeSet,
        reconcile,
        scheduler::{GitReconciler, SyncConfig, SyncService, SyncUpdate},
    },
    tui::{actions::Action, app::App},
    types::{
        diff::DiffOptions,
        repository::{HeadState, Operation, RebaseBackend},
    },
};
use ratatui::layout::Rect;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use support::{assert_read_only, screen_text, tui_app, Repo, TempDir};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

fn ops(repo: &Repo) -> Vec<Operation> {
    assert_read_only(&repo.path, || repo.snapshot().operations)
}

/// `main` and `topic` both change `f.txt` from `base`; `topic` has two
/// commits, so a rebase of `topic` onto `main` has two steps.
fn diverged(label: &str) -> (TempDir, Repo) {
    let (t, repo) = repo(label);
    repo.write("f.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("f.txt", "topic\n");
    repo.commit_all("topic 1");
    repo.write("g.txt", "topic 2\n");
    repo.commit_all("topic 2");
    repo.git(&["switch", "-q", "main"]);
    repo.write("f.txt", "main\n");
    repo.commit_all("main");
    (t, repo)
}

#[test]
fn merge_in_progress_and_aborted() {
    let (_t, repo) = diverged("ops-merge");
    assert!(ops(&repo).is_empty());
    repo.failing_merge("topic");
    assert_eq!(ops(&repo), vec![Operation::Merge]);
    let app = tui_app(&repo.path, 140, 40);
    let text = screen_text(&app);
    assert!(
        text.contains("MERGING") && text.contains("⚠ 1 conflict"),
        "{text}"
    );
    repo.git(&["merge", "--abort"]);
    assert!(ops(&repo).is_empty());
}

#[test]
fn rebase_with_the_merge_backend() {
    let (_t, repo) = diverged("ops-rebase");
    repo.git(&["switch", "-q", "topic"]);
    assert!(!repo.git_output(&["rebase", "main"]).status.success());
    let main = repo.rev("main");
    match &ops(&repo)[..] {
        [Operation::Rebase {
            backend: RebaseBackend::Merge,
            step: Some((1, 2)),
            branch: Some(b),
            onto: Some(o),
        }] => {
            assert_eq!(b, "refs/heads/topic");
            assert_eq!(o.as_str(), main);
        }
        other => panic!("{other:?}"),
    }
    let s = repo.snapshot();
    assert_eq!(
        s.branch.head,
        HeadState::Detached,
        "Git detaches HEAD while rebasing"
    );
    let text = screen_text(&tui_app(&repo.path, 140, 40));
    assert!(text.contains("REBASING 1/2"), "{text}");
    let report = support::stdout(&support::diffkit(&repo.path, &["inspect"]));
    assert!(
        report.contains("A rebase is in progress on topic onto "),
        "{report}"
    );
}

#[test]
fn interactive_rebase_stopped_by_exec_uses_the_same_state() {
    let (_t, repo) = diverged("ops-rebase-i");
    repo.git(&["switch", "-q", "topic"]);
    let out = repo.git_output(&[
        "-c",
        "sequence.editor=true",
        "-c",
        "core.editor=true",
        "rebase",
        "-i",
        "--exec",
        "false",
        "HEAD~1",
    ]);
    assert!(!out.status.success(), "the failing exec stops the rebase");
    assert!(matches!(
        &ops(&repo)[..],
        [Operation::Rebase {
            backend: RebaseBackend::Merge,
            branch: Some(_),
            ..
        }]
    ));
    let text = screen_text(&tui_app(&repo.path, 140, 40));
    // Git counts todo lines: the pick and the failed exec.
    assert!(text.contains("REBASING 2/2"), "{text}");
}

#[test]
fn rebase_with_the_apply_backend() {
    let (_t, repo) = diverged("ops-rebase-apply");
    repo.git(&["switch", "-q", "topic"]);
    assert!(!repo
        .git_output(&["rebase", "--apply", "main"])
        .status
        .success());
    assert!(matches!(
        &ops(&repo)[..],
        [Operation::Rebase {
            backend: RebaseBackend::Apply,
            step: Some((1, 2)),
            ..
        }]
    ));
}

#[test]
fn cherry_pick_and_revert_in_progress() {
    let (_t, repo) = diverged("ops-pick");
    let topic1 = repo.rev("topic~1");
    assert!(!repo.git_output(&["cherry-pick", &topic1]).status.success());
    assert_eq!(ops(&repo), vec![Operation::CherryPick]);
    let text = screen_text(&tui_app(&repo.path, 140, 40));
    assert!(text.contains("CHERRY-PICKING"), "{text}");
    repo.git(&["cherry-pick", "--abort"]);

    // Revert a change that a later commit built on: conflicts.
    repo.write("h.txt", "1\n");
    let first = repo.commit_all("h 1");
    repo.write("h.txt", "2\n");
    repo.commit_all("h 2");
    assert!(!repo
        .git_output(&["revert", "--no-edit", &first])
        .status
        .success());
    assert_eq!(ops(&repo), vec![Operation::Revert]);
    let text = screen_text(&tui_app(&repo.path, 140, 40));
    assert!(text.contains("REVERTING"), "{text}");
}

#[test]
fn bisect_in_progress() {
    let (_t, repo) = repo("ops-bisect");
    for i in 0..5 {
        repo.write("n.txt", &format!("{i}\n"));
        repo.commit_all(&format!("c{i}"));
    }
    repo.git(&["bisect", "start", "HEAD", "HEAD~4"]);
    assert_eq!(
        ops(&repo),
        vec![Operation::Bisect {
            start: Some("main".into())
        }]
    );
    let s = repo.snapshot();
    assert_eq!(s.branch.head, HeadState::Detached);
    let text = screen_text(&tui_app(&repo.path, 140, 40));
    assert!(
        text.contains("BISECTING") && text.contains("HEAD detached at"),
        "{text}"
    );
    repo.git(&["bisect", "reset"]);
    assert!(ops(&repo).is_empty());
}

#[test]
fn operations_are_per_worktree() {
    let (t, repo) = diverged("ops-worktree");
    let wt_path = t.join("wt");
    repo.git(&[
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("add"),
        std::ffi::OsStr::new("-q"),
        std::ffi::OsStr::new("-b"),
        std::ffi::OsStr::new("wt-branch"),
        wt_path.as_os_str(),
        std::ffi::OsStr::new("topic"),
    ]);
    let wt = Repo {
        path: support::real(&wt_path),
    };
    repo.failing_merge("topic");
    assert_eq!(ops(&repo), vec![Operation::Merge]);
    assert!(ops(&wt).is_empty(), "the main worktree's merge is not ours");
    let main_head = repo.rev("main");
    assert!(!wt.git_output(&["cherry-pick", &main_head]).status.success());
    assert_eq!(ops(&wt), vec![Operation::CherryPick]);
    assert_eq!(ops(&repo), vec![Operation::Merge]);
}

#[test]
fn detached_and_unborn_head() {
    let (_t, repo) = repo("ops-heads");
    let mut app = assert_read_only(&repo.path, || tui_app(&repo.path, 120, 30));
    let text = screen_text(&app);
    assert!(text.contains("(no commits yet)"), "{text}");
    app.dispatch(Action::ToggleHistory);
    let text = screen_text(&app);
    assert!(text.contains("No commits yet on this branch."), "{text}");
    repo.write("a.txt", "a\n");
    let first = repo.commit_all("one");
    repo.commit_all("two");
    repo.git(&["checkout", "-q", "--detach", &first]);
    let app = tui_app(&repo.path, 120, 30);
    assert!(screen_text(&app).contains(&format!("HEAD detached at {}", &first[..12])));
    assert!(
        ops(&repo).is_empty(),
        "detached is a HEAD state, not an operation"
    );
}

#[test]
fn missing_git_metadata_is_a_clear_error() {
    let (t, repo) = repo("ops-missing");
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    // A worktree whose .git file points at a directory that is gone.
    let broken = t.join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join(".git"), "gitdir: /nonexistent/diffkit/gitdir\n").unwrap();
    let err = App::open(&broken, Rect::new(0, 0, 100, 30)).unwrap_err();
    assert!(
        err.to_string().contains("not inside a Git repository"),
        "{err}"
    );
    let out = support::diffkit(&broken, &["inspect"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(support::stderr(&out).contains("not inside a Git repository"));
}

#[cfg(unix)]
#[test]
fn unreadable_git_directory_is_an_error_not_a_crash() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, repo) = repo("ops-unreadable");
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    let git_dir = repo.path.join(".git");
    std::fs::set_permissions(&git_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores permissions; the scenario cannot be produced then.
    let blocked = std::fs::read_dir(&git_dir).is_err();
    let out = support::diffkit(&repo.path, &["inspect"]);
    std::fs::set_permissions(&git_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !blocked {
        eprintln!("SKIPPED: permissions are not enforced for this user");
        return;
    }
    assert_eq!(out.status.code(), Some(1), "{}", support::stderr(&out));
    assert!(support::stderr(&out).starts_with("diffkit: error: "));
}

#[test]
fn watcher_publishes_operation_changes() {
    let (_t, repo) = diverged("ops-watch");
    let opts = DiffOptions::default();
    let state = reconcile::reconcile(&repo.path, opts, ChangeSet::STARTUP, None).unwrap();
    let (tx, rx) = mpsc::channel();
    let service = SyncService::start(
        SyncConfig {
            quiet: Duration::from_millis(100),
            ..SyncConfig::default()
        },
        GitReconciler::new(repo.path.clone(), opts, state),
        move |e| {
            let _ = tx.send(e);
        },
    );
    let wait_for = |want: &[Operation]| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {want:?}");
            if let Ok(e) = rx.recv_timeout(left) {
                if let SyncUpdate::Reconciled(r) = e.update {
                    if r.snapshot.operations == want {
                        return;
                    }
                }
            }
        }
    };
    // Wait until the watcher is active (its startup run completes).
    std::thread::sleep(Duration::from_millis(500));
    repo.failing_merge("topic");
    wait_for(&[Operation::Merge]);
    repo.git(&["merge", "--abort"]);
    wait_for(&[]);
    assert!(service.shutdown(Duration::from_secs(5)));
}
