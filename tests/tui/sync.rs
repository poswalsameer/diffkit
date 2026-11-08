//! The interface's handling of synchronization results: atomic state
//! replacement, selection preservation, targeted cache invalidation, sync
//! indicators, degraded mode, and responsiveness during reconciliation.

use crate::support;

use diffkit::{
    git::commands::thread_invocations,
    sync::{
        classifier::WatchLayout,
        events::{ChangeSet, SyncDiagnostics},
        reconcile::{self, Reconciled},
        scheduler::{Reconciler, SyncConfig, SyncEvent, SyncService, SyncUpdate},
    },
    tui::{
        actions::Action,
        app::{App, StatusKind, SyncPhase},
    },
    types::diff::{DiffComparison, DiffOptions},
};
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use support::{numbered, render_buffer, screen, screen_text, tui_app, Repo, TempDir};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, &numbered(&format!("{f} "), 40));
    }
    repo.commit_all("base");
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(
            f,
            &numbered(&format!("{f} "), 40).replace(" 20\n", " twenty\n"),
        );
    }
    (tmp, repo)
}

/// What the background worker would publish after `trigger`.
fn sync(app: &mut App, trigger: ChangeSet) {
    let r = reconcile::reconcile(app.start_path(), DiffOptions::default(), trigger, None).unwrap();
    app.apply_sync(event(SyncUpdate::Reconciled(Box::new(r)), false));
}

fn event(update: SyncUpdate, pending: bool) -> SyncEvent {
    SyncEvent {
        update,
        diagnostics: SyncDiagnostics::default(),
        pending,
    }
}

fn selected(app: &App) -> String {
    app.selected_file()
        .unwrap()
        .path()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn unrelated_changes_keep_selection_and_cached_diffs() {
    let (_t, repo) = repo("tsync-keep");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::SelectLastFile); // c.txt
    app.dispatch(Action::SelectPreviousFile); // b.txt
    assert_eq!(app.cached_entries(), 3);
    repo.write(
        "c.txt",
        &numbered("c.txt ", 40)
            .replace(" 20\n", " twenty\n")
            .replace(" 22\n", " rewritten after loading\n"),
    );
    repo.write("d.txt", "brand new untracked\n");
    sync(&mut app, ChangeSet::WORKTREE);
    assert_eq!(
        selected(&app),
        "b.txt",
        "selection survives another file changing"
    );
    assert_eq!(app.mode(), DiffComparison::Unstaged);
    assert_eq!(
        app.cache_stats().invalidations,
        1,
        "only c.txt's diff was dropped"
    );
    let before = thread_invocations();
    app.load_pending();
    assert_eq!(
        thread_invocations(),
        before,
        "b.txt is still cached and current"
    );
    app.dispatch(Action::SelectNextFile);
    assert_eq!(thread_invocations(), before + 1, "c.txt reloads lazily");
    assert!(
        screen_text(&app).contains("c.txt rewritten after loading"),
        "{}",
        screen_text(&app)
    );
    assert_eq!(
        app.mode_state(DiffComparison::Untracked).files().len(),
        1,
        "counts updated"
    );
}

#[test]
fn the_viewed_file_changing_reloads_it_and_keeps_the_scroll_position() {
    let (_t, repo) = repo("tsync-reload");
    repo.write("long.txt", &numbered("line ", 300));
    repo.git(&["add", "long.txt"]);
    repo.git(&["commit", "-q", "-m", "long"]);
    let edited = |last: &str| {
        numbered("line ", 300)
            .replace("line 10\n", "ten\n")
            .replace("line 100\n", "one hundred\n")
            .replace("line 200\n", last)
            .replace("line 290\n", "two ninety\n")
    };
    repo.write("long.txt", &edited("two hundred\n"));
    let mut app = tui_app(&repo.path, 120, 30);
    while selected(&app) != "long.txt" {
        app.dispatch(Action::SelectNextFile);
    }
    app.dispatch(Action::NextHunk);
    let top = app.scroll().top;
    assert!(top > 0);
    repo.write("long.txt", &edited("two hundred and one\n"));
    sync(&mut app, ChangeSet::WORKTREE);
    app.load_pending();
    assert_eq!(selected(&app), "long.txt");
    assert_eq!(app.scroll().top, top, "same file: position kept");
    app.dispatch(Action::NextHunk);
    assert!(
        screen_text(&app).contains("two hundred and one"),
        "no stale content shown"
    );
}

#[test]
fn a_vanished_selection_moves_to_the_nearest_file_with_a_notice() {
    let (_t, repo) = repo("tsync-vanish");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::SelectNextFile); // b.txt
    repo.git(&["checkout", "-q", "--", "b.txt"]);
    sync(&mut app, ChangeSet::WORKTREE);
    assert_eq!(selected(&app), "c.txt", "same position, next file");
    let status = app.status().unwrap();
    assert_eq!(status.kind, StatusKind::Info);
    assert!(
        status.text.contains("b.txt is no longer in UNSTAGED"),
        "{}",
        status.text
    );
    repo.git(&["checkout", "-q", "--", "a.txt", "c.txt"]);
    sync(&mut app, ChangeSet::WORKTREE);
    assert!(app.selected_file().is_none());
    assert!(screen_text(&app).contains("No unstaged changes"));
}

#[test]
fn header_follows_branch_switches_and_commits() {
    let (_t, repo) = repo("tsync-header");
    let mut app = tui_app(&repo.path, 140, 30);
    assert!(screen(&app)[0].contains("⎇ main"));
    repo.git(&["stash", "-q"]);
    repo.git(&["switch", "-q", "-c", "feature/sync"]);
    sync(&mut app, ChangeSet::HEAD);
    assert!(
        screen(&app)[0].contains("⎇ feature/sync"),
        "{}",
        screen(&app)[0]
    );
    assert!(app.mode_state(DiffComparison::Unstaged).files().is_empty());
}

#[test]
fn sync_indicator_states() {
    let (_t, repo) = repo("tsync-indicator");
    let mut app = tui_app(&repo.path, 120, 30);
    let indicator = |app: &App| screen(app)[28].clone();
    assert_eq!(app.sync_state().phase, SyncPhase::Manual);
    app.set_background_sync(true);
    app.apply_sync(event(SyncUpdate::Watching { roots: 1 }, false));
    app.apply_sync(event(SyncUpdate::Unchanged, false));
    assert!(
        indicator(&app).ends_with("● Synced"),
        "{:?}",
        indicator(&app)
    );
    app.apply_sync(event(SyncUpdate::ChangesDetected, true));
    assert!(indicator(&app).contains("◌ Changes detected"));
    app.apply_sync(event(SyncUpdate::Synchronizing, false));
    assert!(indicator(&app).contains("↻ Synchronizing"));
    app.apply_sync(event(SyncUpdate::Unchanged, true));
    assert_eq!(
        app.sync_state().phase,
        SyncPhase::ChangesDetected,
        "more changes pending"
    );
    app.apply_sync(event(
        SyncUpdate::Failed("repository vanished".into()),
        false,
    ));
    assert!(indicator(&app).contains("! Refresh required"));
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
}

#[test]
fn degraded_mode_keeps_manual_refresh_through_the_worker() {
    let (_t, repo) = repo("tsync-degraded");
    let mut app = tui_app(&repo.path, 120, 30);
    app.set_background_sync(true);
    app.apply_sync(event(
        SyncUpdate::WatcherUnavailable("filesystem watcher failed: too many watches".into()),
        false,
    ));
    let text = screen_text(&app);
    assert!(text.contains("! Watcher unavailable"), "{text}");
    assert!(text.contains("Press r to refresh manually"));
    let before = thread_invocations();
    app.dispatch(Action::Refresh);
    assert!(app.take_refresh_request(), "handed to the worker");
    assert!(!app.take_refresh_request());
    assert_eq!(thread_invocations(), before, "no inline Git work");
    let r = reconcile::reconcile(
        app.start_path(),
        DiffOptions::default(),
        ChangeSet::MANUAL,
        None,
    )
    .unwrap();
    app.apply_sync(event(SyncUpdate::Reconciled(Box::new(r)), false));
    assert!(app.status().unwrap().text.starts_with("Refreshed:"));
    assert!(
        screen_text(&app).contains("! Watcher unavailable"),
        "still degraded after a manual refresh"
    );
}

#[test]
fn no_watch_mode_is_labelled() {
    let (_t, repo) = repo("tsync-nowatch");
    let mut app = tui_app(&repo.path, 120, 30);
    app.set_background_sync(true);
    app.apply_sync(event(
        SyncUpdate::WatcherUnavailable("automatic sync is off (--no-watch)".into()),
        false,
    ));
    app.apply_sync(event(SyncUpdate::Unchanged, false));
    assert!(screen_text(&app).contains("○ Auto-sync off"));
}

#[test]
fn cache_budget_evicts_but_protects_the_shown_diff() {
    let (_t, repo) = repo("tsync-budget");
    let mut app = tui_app(&repo.path, 120, 40);
    let one = app.cache_stats().bytes;
    assert!(one > 0);
    app.set_cache_budget(one + one / 2); // room for about one diff
    app.dispatch(Action::SelectNextFile);
    app.dispatch(Action::SelectNextFile);
    let s = app.cache_stats();
    assert!(s.evictions >= 2, "{s:?}");
    assert!(app.view().is_some(), "the shown diff is never evicted");
    assert!(s.bytes <= s.budget_bytes.max(one * 2));
    let before = thread_invocations();
    app.dispatch(Action::SelectFirstFile);
    assert_eq!(
        thread_invocations(),
        before + 1,
        "evicted diff reloads on demand"
    );
    assert!(app.cache_stats().misses >= 3);
}

#[test]
fn diagnostics_overlay_renders() {
    let (_t, repo) = repo("tsync-diag");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::ToggleDiagnostics);
    let text = screen_text(&app);
    assert!(text.contains("Diagnostics — D or Esc to close"), "{text}");
    assert!(text.contains("hits / misses") && text.contains("reconciliations requested"));
    app.dispatch(Action::ToggleDiagnostics);
    assert!(app.overlay().is_none());
}

/// A reconciler that holds every run until released.
struct Blocking {
    template: Reconciled,
    gate: Receiver<()>,
}

impl Reconciler for Blocking {
    fn run(&mut self, trigger: ChangeSet) -> Result<Option<Box<Reconciled>>, String> {
        let _ = self.gate.recv();
        Ok(Some(Box::new(Reconciled {
            trigger,
            ..self.template.clone()
        })))
    }

    fn layout(&mut self, _: bool) -> Option<WatchLayout> {
        None
    }
}

#[test]
fn the_interface_stays_responsive_during_reconciliation() {
    let (_t, repo) = repo("tsync-responsive");
    let mut app = tui_app(&repo.path, 120, 40);
    let (gate_tx, gate_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let service = SyncService::start(
        SyncConfig {
            watch: false,
            ..SyncConfig::default()
        },
        Blocking {
            template: app.reconciled(),
            gate: gate_rx,
        },
        move |e| {
            let _ = tx.send(e);
        },
    );
    app.set_background_sync(true);
    app.dispatch(Action::Refresh);
    assert!(app.take_refresh_request());
    service.request_refresh();
    // Wait until the worker is inside the (blocked) reconciliation.
    loop {
        let e = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("worker running");
        let synchronizing = matches!(e.update, SyncUpdate::Synchronizing);
        app.apply_sync(e);
        if synchronizing {
            break;
        }
    }
    let began = Instant::now();
    for i in 0..200 {
        app.dispatch(if i % 2 == 0 {
            Action::SelectNextFile
        } else {
            Action::PageDown
        });
        render_buffer(&app);
    }
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "input and rendering are not blocked"
    );
    assert!(screen_text(&app).contains("↻ Synchronizing"));
    gate_tx.send(()).unwrap();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(5)).expect("result");
        let done = matches!(e.update, SyncUpdate::Reconciled(_));
        app.apply_sync(e);
        if done {
            break;
        }
    }
    assert_eq!(app.sync_state().phase, SyncPhase::Synced);
    // This worker runs with watching disabled, which the indicator says.
    assert!(screen_text(&app).contains("○ Auto-sync off"));
    assert!(app.status().unwrap().text.starts_with("Refreshed:"));
    drop(gate_tx);
    assert!(service.shutdown(Duration::from_secs(2)));
}

#[test]
fn a_load_finishing_after_its_file_changed_is_not_shown() {
    let (_t, repo) = repo("tsync-late-load");
    let mut app = tui_app(&repo.path, 120, 40);
    app.apply(Action::SelectNextFile);
    assert_eq!(selected(&app), "b.txt");
    // The load starts (as on the loader thread), then b.txt changes and a
    // reconciliation is applied before the load finishes.
    let job = app.take_load().expect("b.txt is not cached");
    assert!(app.take_load().is_none(), "taken once");
    repo.write("b.txt", "rewritten while loading\n");
    let r = reconcile::reconcile(
        app.start_path(),
        DiffOptions::default(),
        ChangeSet::MANUAL,
        None,
    )
    .unwrap();
    app.apply_reconciled(r);
    app.finish_load(job.run());
    assert!(app.loaded().is_none(), "the older version is dropped");
    assert!(app.is_loading());
    app.load_pending();
    let v = app.view().expect("reloaded");
    assert_eq!((v.added, v.removed), (1, 40), "the current version");
}

#[test]
fn a_finished_load_for_another_file_is_cached_without_moving_the_view() {
    let (_t, repo) = repo("tsync-other-load");
    let mut app = tui_app(&repo.path, 120, 40);
    app.apply(Action::SelectNextFile);
    let job = app.take_load().expect("b.txt is not cached");
    app.apply(Action::SelectNextFile);
    assert_eq!(selected(&app), "c.txt");
    let cached = app.cached_entries();
    app.finish_load(job.run());
    assert_eq!(app.cached_entries(), cached + 1, "b.txt is cached");
    assert!(app.loaded().is_none(), "c.txt is still pending");
    app.load_pending();
    assert!(screen_text(&app).contains("c.txt twenty"));
}
