//! Paths the platform allows but tools often break on: a repository and a
//! linked worktree under directories with spaces and non-ASCII names,
//! files with Unicode names, through the interface, the CLI, and the real
//! filesystem watcher.

mod common;

use common::{screen_text, tui_app, Repo, TempDir};
use diffkit::{
    diff::{DiffComparison, DiffOptions},
    sync::{reconcile, ChangeSet, GitReconciler, SyncConfig, SyncService, SyncUpdate},
    tui::Action,
};
use std::{
    ffi::OsStr,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

fn unstaged_paths(r: &diffkit::sync::Reconciled) -> Vec<String> {
    r.lists[1]
        .as_ref()
        .unwrap()
        .files
        .iter()
        .map(|f| f.path().to_string_lossy().into_owned())
        .collect()
}

/// Watches `path` and waits until the unstaged list equals `want`.
fn watch_until(path: &Path, change: impl FnOnce(), want: &[&str]) {
    let opts = DiffOptions::default();
    let state = reconcile::reconcile(path, opts, ChangeSet::STARTUP, None).unwrap();
    let (tx, rx) = mpsc::channel();
    let service = SyncService::start(
        SyncConfig {
            quiet: Duration::from_millis(100),
            ..SyncConfig::default()
        },
        GitReconciler::new(path.to_path_buf(), opts, state),
        move |e| {
            let _ = tx.send(e);
        },
    );
    // Let the watcher start (its startup reconciliation completes first).
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let e = rx.recv_timeout(deadline - Instant::now()).expect("startup");
        if matches!(e.update, SyncUpdate::Reconciled(_) | SyncUpdate::Unchanged) {
            break;
        }
    }
    change();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "timed out waiting for {want:?}");
        if let Ok(e) = rx.recv_timeout(left) {
            if let SyncUpdate::Reconciled(r) = e.update {
                if unstaged_paths(&r) == want {
                    break;
                }
            }
        }
    }
    assert!(service.shutdown(Duration::from_secs(5)));
}

#[test]
fn repository_under_spaces_and_non_ascii_directories() {
    let tmp = TempDir::new("paths ü 日本");
    let repo = Repo::init(tmp.join("my repo — café 🎉"));
    repo.write("dir é/ñame with space.txt", "one\n");
    repo.write("日本語/ファイル.txt", "two\n");
    repo.commit_all("base");
    repo.write("日本語/ファイル.txt", "two\nmore\n");

    let mut app = tui_app(&repo.path, 140, 30);
    app.dispatch(Action::SetComparison(DiffComparison::Unstaged));
    let text = screen_text(&app);
    assert!(text.contains("my repo — café 🎉"), "{text}");
    // Double-width characters occupy two cells; the second is blank in
    // the test buffer.
    let compact = text.replace(' ', "");
    assert!(
        compact.contains("日本語/ファイル.txt") && compact.contains("+more"),
        "{text}"
    );

    let out = common::diffkit(&repo.path.join("dir é"), &["diff-stats"]);
    assert!(out.status.success(), "{}", common::stderr(&out));
    assert!(common::stdout(&out).contains("日本語/ファイル.txt"));

    watch_until(
        &repo.path,
        || repo.write("dir é/ñame with space.txt", "one\nedited\n"),
        &["dir é/ñame with space.txt", "日本語/ファイル.txt"],
    );
}

#[test]
fn linked_worktree_under_a_non_ascii_path() {
    let tmp = TempDir::new("paths-wt");
    let repo = Repo::init(tmp.join("main repo"));
    repo.write("a.txt", "a\n");
    repo.commit_all("base");
    let wt_path = tmp.join("arbre de travail ü");
    repo.git(&[
        OsStr::new("worktree"),
        OsStr::new("add"),
        OsStr::new("-q"),
        OsStr::new("-b"),
        OsStr::new("ü-branch"),
        wt_path.as_os_str(),
    ]);
    let wt = common::real(&wt_path);
    let app = tui_app(&wt, 140, 30);
    let text = screen_text(&app);
    assert!(
        text.contains("arbre de travail ü") && text.contains("[worktree]"),
        "{text}"
    );
    assert!(text.contains("⎇ ü-branch"), "{text}");
    let wt_repo = Repo { path: wt.clone() };
    watch_until(&wt, || wt_repo.write("a.txt", "a\nwt\n"), &["a.txt"]);
}
