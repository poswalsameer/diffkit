//! The synchronization worker with a scripted reconciler: debouncing,
//! no overlapping runs, events during a run, manual refresh, failures,
//! overflow, and shutdown. No filesystem watcher is involved.

mod common;

use common::{Repo, TempDir};
use diffkit::{
    diff::DiffOptions,
    sync::{
        classifier::WatchLayout,
        reconcile::{self, Reconciled},
        ChangeSet, Reconciler, SyncConfig, SyncEvent, SyncService, SyncUpdate,
    },
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

fn template() -> (TempDir, Reconciled) {
    let tmp = TempDir::new("sched");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a", "a\n");
    repo.commit_all("a");
    common::isolate();
    let r =
        reconcile::reconcile(&repo.path, DiffOptions::default(), ChangeSet::STARTUP, None).unwrap();
    (tmp, r)
}

#[derive(Clone, Default)]
struct Probe {
    triggers: Arc<Mutex<Vec<ChangeSet>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

impl Probe {
    fn runs(&self) -> Vec<ChangeSet> {
        self.triggers.lock().unwrap().clone()
    }
}

struct Fake {
    template: Reconciled,
    probe: Probe,
    /// Each run waits for one token when set.
    gate: Option<Receiver<()>>,
    started: Option<Sender<()>>,
    /// Results to return, in order; afterwards `Ok(Some(..))`.
    script: Vec<Result<bool, String>>,
}

impl Reconciler for Fake {
    fn run(&mut self, trigger: ChangeSet) -> Result<Option<Box<Reconciled>>, String> {
        let now = self.probe.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.probe.max_active.fetch_max(now, Ordering::SeqCst);
        self.probe.triggers.lock().unwrap().push(trigger);
        if let Some(s) = &self.started {
            let _ = s.send(());
        }
        if let Some(g) = &self.gate {
            let _ = g.recv();
        }
        self.probe.active.fetch_sub(1, Ordering::SeqCst);
        let changed = if self.script.is_empty() {
            Ok(true)
        } else {
            self.script.remove(0)
        };
        match changed {
            Ok(true) => Ok(Some(Box::new(Reconciled {
                trigger,
                ..self.template.clone()
            }))),
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn layout(&mut self, _: bool) -> Option<WatchLayout> {
        None
    }
}

fn config(quiet_ms: u64, max_ms: u64) -> SyncConfig {
    SyncConfig {
        quiet: Duration::from_millis(quiet_ms),
        max_delay: Duration::from_millis(max_ms),
        watch: false,
        ..SyncConfig::default()
    }
}

fn start(config: SyncConfig, fake: Fake) -> (SyncService, Receiver<SyncEvent>) {
    let (tx, rx) = mpsc::channel();
    let service = SyncService::start(config, fake, move |e| {
        let _ = tx.send(e);
    });
    (service, rx)
}

/// Waits for the next completed (or failed) reconciliation.
fn next_result(rx: &Receiver<SyncEvent>, timeout: Duration) -> Option<SyncEvent> {
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(e)
                if matches!(
                    e.update,
                    SyncUpdate::Reconciled(_) | SyncUpdate::Unchanged | SyncUpdate::Failed(_)
                ) =>
            {
                return Some(e)
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn fake(template: &Reconciled, probe: &Probe) -> Fake {
    Fake {
        template: template.clone(),
        probe: probe.clone(),
        gate: None,
        started: None,
        script: Vec::new(),
    }
}

#[test]
fn a_burst_of_events_runs_one_reconciliation() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (service, rx) = start(config(80, 5000), fake(&tpl, &probe));
    for i in 0..100 {
        let c = if i % 10 == 0 {
            ChangeSet::INDEX
        } else {
            ChangeSet::WORKTREE
        };
        assert!(service.inject(c));
    }
    let e = next_result(&rx, Duration::from_secs(5)).expect("reconciled");
    assert!(
        next_result(&rx, Duration::from_millis(400)).is_none(),
        "no second run"
    );
    assert_eq!(probe.runs(), vec![ChangeSet::WORKTREE | ChangeSet::INDEX]);
    let d = e.diagnostics;
    assert_eq!(
        (d.relevant_events, d.coalesced_events, d.reconcile_requests),
        (100, 99, 1)
    );
    assert_eq!(d.reconciles_completed, 1);
    assert!(!e.pending);
    assert!(service.shutdown(Duration::from_secs(2)));
}

#[test]
fn events_during_a_run_are_kept_and_runs_never_overlap() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (gate_tx, gate_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let mut f = fake(&tpl, &probe);
    f.gate = Some(gate_rx);
    f.started = Some(started_tx);
    let (service, rx) = start(config(30, 5000), f);
    service.inject(ChangeSet::WORKTREE);
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first run started");
    for _ in 0..20 {
        service.inject(ChangeSet::INDEX);
    }
    service.request_refresh();
    gate_tx.send(()).unwrap();
    let first = next_result(&rx, Duration::from_secs(5)).unwrap();
    assert!(
        first.pending,
        "changes that arrived during the run are reported as pending"
    );
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("second run started");
    gate_tx.send(()).unwrap();
    let second = next_result(&rx, Duration::from_secs(5)).unwrap();
    assert!(!second.pending);
    let runs = probe.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0], ChangeSet::WORKTREE);
    assert!(
        runs[1].contains(ChangeSet::INDEX | ChangeSet::MANUAL),
        "{:?}",
        runs[1]
    );
    assert_eq!(
        probe.max_active.load(Ordering::SeqCst),
        1,
        "never two at once"
    );
    drop(gate_tx);
    assert!(service.shutdown(Duration::from_secs(2)));
}

#[test]
fn manual_refresh_skips_the_debounce_delay() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (service, rx) = start(config(60_000, 120_000), fake(&tpl, &probe));
    service.inject(ChangeSet::WORKTREE);
    let began = Instant::now();
    service.request_refresh();
    next_result(&rx, Duration::from_secs(5)).expect("ran without waiting a minute");
    assert!(began.elapsed() < Duration::from_secs(5));
    assert_eq!(
        probe.runs(),
        vec![ChangeSet::WORKTREE | ChangeSet::MANUAL],
        "pending changes included"
    );
}

#[test]
fn continuous_activity_cannot_postpone_reconciliation_forever() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (service, _rx) = start(config(300, 500), fake(&tpl, &probe));
    let began = Instant::now();
    // An event every 50 ms: the 300 ms quiet period never elapses.
    while began.elapsed() < Duration::from_millis(2500) {
        service.inject(ChangeSet::WORKTREE);
        std::thread::sleep(Duration::from_millis(50));
    }
    let runs = probe.runs().len();
    assert!(
        runs >= 2,
        "maximum delay forces runs during activity, got {runs}"
    );
}

#[test]
fn failures_are_reported_and_the_next_change_retries() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let mut f = fake(&tpl, &probe);
    f.script = vec![Err("git exploded".into()), Ok(false)];
    let (service, rx) = start(config(20, 1000), f);
    service.inject(ChangeSet::WORKTREE);
    let e = next_result(&rx, Duration::from_secs(5)).unwrap();
    assert!(matches!(&e.update, SyncUpdate::Failed(m) if m == "git exploded"));
    assert_eq!(e.diagnostics.reconcile_failures, 1);
    assert!(
        next_result(&rx, Duration::from_millis(300)).is_none(),
        "no automatic retry loop"
    );
    service.inject(ChangeSet::WORKTREE);
    let e = next_result(&rx, Duration::from_secs(5)).unwrap();
    assert!(matches!(e.update, SyncUpdate::Unchanged));
    assert_eq!(e.diagnostics.reconciles_unchanged, 1);
}

#[test]
fn dropped_events_force_a_full_overflow_reconciliation() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (service, rx) = start(config(20, 1000), fake(&tpl, &probe));
    service.simulate_dropped_events(37);
    let e = next_result(&rx, Duration::from_secs(5)).unwrap();
    assert_eq!(probe.runs(), vec![ChangeSet::OVERFLOW]);
    assert_eq!(
        (e.diagnostics.overflows, e.diagnostics.dropped_events),
        (1, 37)
    );
}

#[test]
fn shutdown_during_debounce_is_prompt_and_runs_nothing() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (service, _rx) = start(config(60_000, 120_000), fake(&tpl, &probe));
    service.inject(ChangeSet::WORKTREE);
    std::thread::sleep(Duration::from_millis(50));
    let began = Instant::now();
    assert!(service.shutdown(Duration::from_secs(2)));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert!(probe.runs().is_empty());
}

#[test]
fn shutdown_never_hangs_on_a_busy_worker() {
    let (_t, tpl) = template();
    let probe = Probe::default();
    let (gate_tx, gate_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let mut f = fake(&tpl, &probe);
    f.gate = Some(gate_rx);
    f.started = Some(started_tx);
    let (service, _rx) = start(config(10, 100), f);
    service.request_refresh();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let began = Instant::now();
    assert!(
        !service.shutdown(Duration::from_millis(200)),
        "detached instead of waiting"
    );
    assert!(began.elapsed() < Duration::from_secs(1));
    gate_tx.send(()).unwrap(); // let the detached worker finish
}

/// Returns an unsettled result first (Git kept changing), then settled ones.
struct Unsettled {
    template: Reconciled,
    runs: Arc<Mutex<Vec<ChangeSet>>>,
}

impl Reconciler for Unsettled {
    fn run(&mut self, trigger: ChangeSet) -> Result<Option<Box<Reconciled>>, String> {
        let mut runs = self.runs.lock().unwrap();
        runs.push(trigger);
        Ok(Some(Box::new(Reconciled {
            trigger,
            settled: runs.len() > 1,
            ..self.template.clone()
        })))
    }

    fn layout(&mut self, _: bool) -> Option<WatchLayout> {
        None
    }
}

#[test]
fn unsettled_results_are_withheld_and_rerun_in_full() {
    let (_t, tpl) = template();
    let runs = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    let service = SyncService::start(
        config(30, 1000),
        Unsettled {
            template: tpl,
            runs: Arc::clone(&runs),
        },
        move |e| {
            let _ = tx.send(e);
        },
    );
    service.inject(ChangeSet::WORKTREE);
    let mut published = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while published.is_empty() && Instant::now() < deadline {
        if let Ok(e) = rx.recv_timeout(Duration::from_millis(100)) {
            if let SyncUpdate::Reconciled(r) = e.update {
                assert_eq!(e.diagnostics.reconciles_withheld, 1);
                published.push(r);
            }
        }
    }
    assert_eq!(published.len(), 1, "only the settled result is published");
    assert!(published[0].settled);
    let runs = runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 2);
    assert!(
        runs[1].contains(ChangeSet::INDEX | ChangeSet::HEAD),
        "the follow-up run rediscovers everything: {:?}",
        runs[1]
    );
    assert!(service.shutdown(Duration::from_secs(2)));
}
