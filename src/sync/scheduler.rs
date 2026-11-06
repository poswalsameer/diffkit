//! The synchronization worker: one thread per repository that owns the
//! watcher, classifies and debounces events, and runs reconciliations one
//! at a time.
//!
//! Concurrency:
//! - Reconciliation runs on the worker thread itself, so two can never
//!   overlap.
//! - Events arriving meanwhile wait in the bounded queue. They are drained
//!   before the result is published, and start the next debounce window,
//!   so they are never lost.
//! - The interface thread only receives finished [`SyncEvent`]s.
//!
//! Shutdown: a flag plus a message wake the worker. [`SyncService::shutdown`]
//! waits a bounded time and otherwise detaches the thread rather than
//! blocking exit; the thread ends after its current Git command.

use super::{
    classifier::{classify, WatchLayout},
    debounce::{Debouncer, DEFAULT_MAX_DELAY, DEFAULT_QUIET},
    events::{ChangeSet, SyncDiagnostics},
    reconcile::{self, Reconciled},
    watcher::{self, FsWatcher},
};
use crate::{diff::DiffOptions, git::commands::thread_invocations};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Messages to the worker.
pub enum Msg {
    Fs(notify::Result<notify::Event>),
    /// Pre-classified changes (tests and benchmarks inject these).
    Changes(ChangeSet),
    /// Wake up: a manual refresh or shutdown flag was set.
    Wake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncConfig {
    pub quiet: Duration,
    pub max_delay: Duration,
    /// Bound of the event queue between the watcher and the worker.
    pub queue_capacity: usize,
    /// Whether to watch the filesystem at all (`--no-watch` disables it).
    pub watch: bool,
    /// Consecutive automatic watcher restarts before giving up.
    pub max_watch_restarts: u32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            quiet: DEFAULT_QUIET,
            max_delay: DEFAULT_MAX_DELAY,
            queue_capacity: 4096,
            watch: true,
            max_watch_restarts: 3,
        }
    }
}

/// Backoff between automatic watcher restart attempts.
const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

#[derive(Debug, Clone)]
pub enum SyncUpdate {
    /// The watcher is running over this many roots.
    Watching { roots: usize },
    /// Automatic synchronization is off (failed or disabled); manual
    /// refresh still works.
    WatcherUnavailable(String),
    /// A relevant change was seen; a reconciliation is pending.
    ChangesDetected,
    /// A reconciliation started.
    Synchronizing,
    /// A new repository state, ready to publish as a whole.
    Reconciled(Box<Reconciled>),
    /// A reconciliation found the same state as before.
    Unchanged,
    /// A reconciliation failed; the previous state is kept.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct SyncEvent {
    pub update: SyncUpdate,
    pub diagnostics: SyncDiagnostics,
    /// More relevant changes are already pending.
    pub pending: bool,
}

/// Produces reconciliation results. The real implementation is
/// [`GitReconciler`]; tests substitute their own.
pub trait Reconciler: Send + 'static {
    /// Runs one reconciliation. `Ok(None)` means the state is unchanged.
    fn run(&mut self, trigger: ChangeSet) -> Result<Option<Box<Reconciled>>, String>;
    /// The current watch layout. `refresh_exclusions` asks to recompute the
    /// ignored directories (one Git call).
    fn layout(&mut self, refresh_exclusions: bool) -> Option<WatchLayout>;
}

/// Reconciles through Git, starting from an already published state.
pub struct GitReconciler {
    start: PathBuf,
    options: DiffOptions,
    last: Reconciled,
    excluded: Option<Vec<PathBuf>>,
}

impl GitReconciler {
    pub fn new(start: PathBuf, options: DiffOptions, current: Reconciled) -> Self {
        Self {
            start,
            options,
            last: current,
            excluded: None,
        }
    }
}

impl Reconciler for GitReconciler {
    fn run(&mut self, trigger: ChangeSet) -> Result<Option<Box<Reconciled>>, String> {
        let r = reconcile::reconcile(&self.start, self.options, trigger, Some(&self.last))
            .map_err(|e| e.to_string())?;
        if r.settled && r.same_state(&self.last) {
            return Ok(None);
        }
        // An unsettled result is never published, so it must not become
        // the base that later results reuse lists from.
        if r.settled {
            self.last = r.clone();
        }
        Ok(Some(Box::new(r)))
    }

    fn layout(&mut self, refresh_exclusions: bool) -> Option<WatchLayout> {
        if refresh_exclusions || self.excluded.is_none() {
            // On failure, exclude nothing: more events, never fewer.
            self.excluded = Some(
                reconcile::ignored_directories(&self.last.snapshot.repository.root)
                    .unwrap_or_default(),
            );
        }
        Some(reconcile::layout_for(
            &self.last,
            self.excluded.clone().unwrap_or_default(),
        ))
    }
}

/// Handle to a running worker.
pub struct SyncService {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    refresh: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    done: Receiver<()>,
}

impl SyncService {
    pub fn start<R: Reconciler>(
        config: SyncConfig,
        reconciler: R,
        sink: impl FnMut(SyncEvent) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel(config.queue_capacity.max(1));
        let (done_tx, done) = mpsc::channel();
        let refresh = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let worker = Worker {
            config,
            rx,
            tx: tx.clone(),
            dropped: Arc::clone(&dropped),
            refresh: Arc::clone(&refresh),
            stop: Arc::clone(&stop),
            debounce: Debouncer::new(config.quiet, config.max_delay),
            immediate: ChangeSet::NONE,
            reconciler,
            sink: Box::new(sink),
            diag: SyncDiagnostics::default(),
            layout: None,
            watcher: None,
            rebuild: false,
            restart_failures: 0,
            restart_at: None,
        };
        let handle = std::thread::Builder::new()
            .name("diffkit-sync".into())
            .spawn(move || {
                worker.run();
                let _ = done_tx.send(());
            })
            .expect("spawn sync worker");
        Self {
            tx,
            dropped,
            refresh,
            stop,
            handle: Some(handle),
            done,
        }
    }

    /// Simulates the watcher dropping `n` events on a full queue, then
    /// wakes the worker (for tests of the overflow path).
    pub fn simulate_dropped_events(&self, n: u64) {
        self.dropped.fetch_add(n, Ordering::Relaxed);
        let _ = self.tx.try_send(Msg::Wake);
    }

    /// Asks for a reconciliation now, bypassing the debounce delay.
    pub fn request_refresh(&self) {
        self.refresh.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(Msg::Wake);
    }

    /// Feeds classified changes as if the watcher had reported them.
    pub fn inject(&self, changes: ChangeSet) -> bool {
        self.tx.try_send(Msg::Changes(changes)).is_ok()
    }

    /// Stops the worker, waiting at most `timeout`. Returns false if the
    /// worker was still busy and was detached instead.
    pub fn shutdown(mut self, timeout: Duration) -> bool {
        self.stop_and_wait(timeout)
    }

    fn stop_and_wait(&mut self, timeout: Duration) -> bool {
        let Some(handle) = self.handle.take() else {
            return true;
        };
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(Msg::Wake);
        match self.done.recv_timeout(timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = handle.join();
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
        }
    }
}

impl Drop for SyncService {
    fn drop(&mut self) {
        self.stop_and_wait(Duration::from_secs(2));
    }
}

struct Worker<R> {
    config: SyncConfig,
    rx: Receiver<Msg>,
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    refresh: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    debounce: Debouncer,
    /// Changes to reconcile without waiting (manual refresh, startup).
    immediate: ChangeSet,
    reconciler: R,
    sink: Box<dyn FnMut(SyncEvent) + Send>,
    diag: SyncDiagnostics,
    layout: Option<WatchLayout>,
    watcher: Option<FsWatcher>,
    /// The watcher reported an error; rebuild after the next reconciliation.
    rebuild: bool,
    restart_failures: u32,
    restart_at: Option<Instant>,
}

impl<R: Reconciler> Worker<R> {
    fn emit(&mut self, update: SyncUpdate) {
        let event = SyncEvent {
            update,
            diagnostics: self.diag,
            pending: self.debounce.is_pending() || !self.immediate.is_empty(),
        };
        (self.sink)(event);
    }

    fn run(mut self) {
        if self.config.watch {
            let before = thread_invocations();
            let layout = self.reconciler.layout(true);
            self.diag.git_calls += thread_invocations() - before;
            if let Some(layout) = layout {
                self.start_watcher(layout);
                // Catch changes made between the initial state and the
                // watcher becoming active.
                self.immediate.insert(ChangeSet::STARTUP);
            }
        } else {
            self.emit(SyncUpdate::WatcherUnavailable(
                "automatic sync is off (--no-watch)".into(),
            ));
        }
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            let wait = [self.debounce.deadline(), self.restart_at]
                .into_iter()
                .flatten()
                .min();
            let msg = if !self.immediate.is_empty() || self.refresh.load(Ordering::SeqCst) {
                match self.rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else if let Some(deadline) = wait {
                match self
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                }
            };
            if let Some(m) = msg {
                self.handle(m);
            }
            if !self.drain() {
                return;
            }
            if self.restart_at.is_some_and(|t| t <= Instant::now()) {
                self.restart_at = None;
                if let Some(layout) = self.reconciler.layout(false) {
                    self.start_watcher(layout);
                }
            }
            let mut trigger = std::mem::take(&mut self.immediate);
            if self.refresh.swap(false, Ordering::SeqCst) {
                trigger.insert(ChangeSet::MANUAL);
                // A manual refresh also retries a watcher that gave up.
                self.restart_failures = 0;
                self.rebuild |= self.config.watch && self.watcher.is_none();
            }
            let batch = if trigger.is_empty() {
                self.debounce.take_if_due(Instant::now()).map(|b| b.changes)
            } else {
                // Everything pending is covered by this reconciliation.
                Some(
                    self.debounce
                        .take()
                        .map_or(trigger, |b| b.changes | trigger),
                )
            };
            if let Some(changes) = batch {
                if self.stop.load(Ordering::SeqCst) {
                    return;
                }
                self.reconcile(changes);
            }
        }
    }

    /// Handles everything already queued. Returns false if disconnected.
    fn drain(&mut self) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(m) => self.handle(m),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        let lost = self.dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            self.diag.overflows += 1;
            self.diag.dropped_events += lost;
            self.record(ChangeSet::OVERFLOW);
        }
        true
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Fs(Ok(event)) => {
                self.diag.fs_events += 1;
                let changes = match &self.layout {
                    Some(layout) => classify(layout, &event),
                    None => ChangeSet::NONE,
                };
                if !changes.is_empty() {
                    self.diag.relevant_events += 1;
                    self.record(changes);
                }
            }
            Msg::Fs(Err(_)) => {
                self.diag.watcher_errors += 1;
                self.rebuild = true;
                self.record(ChangeSet::OVERFLOW);
            }
            Msg::Changes(changes) => {
                self.diag.fs_events += 1;
                self.diag.relevant_events += 1;
                self.record(changes);
            }
            Msg::Wake => {}
        }
    }

    fn record(&mut self, changes: ChangeSet) {
        if self.debounce.record(changes, Instant::now()) {
            self.emit(SyncUpdate::ChangesDetected);
        } else {
            self.diag.coalesced_events += 1;
        }
    }

    fn reconcile(&mut self, trigger: ChangeSet) {
        self.diag.reconcile_requests += 1;
        self.emit(SyncUpdate::Synchronizing);
        let calls = thread_invocations();
        let began = Instant::now();
        let result = self.reconciler.run(trigger);
        let elapsed = began.elapsed();
        self.diag.last_reconcile = Some(elapsed);
        self.diag.total_reconcile += elapsed;
        // Events that arrived during the run start the next window before
        // this result is published, so `pending` is accurate.
        self.drain();
        let update = match result {
            Ok(Some(r)) if !r.settled => {
                // Git kept changing through every attempt, so the result may
                // mix two states. It is withheld (the interface keeps the
                // last consistent state) and a full run follows once Git
                // goes quiet.
                self.diag.reconciles_withheld += 1;
                self.diag.reconcile_retries += r.retries as u64;
                self.debounce
                    .record(ChangeSet::INDEX | ChangeSet::HEAD, Instant::now());
                SyncUpdate::ChangesDetected
            }
            Ok(Some(r)) => {
                self.diag.reconciles_completed += 1;
                self.diag.lists_reused += r.reused as u64;
                self.diag.reconcile_retries += r.retries as u64;
                SyncUpdate::Reconciled(r)
            }
            Ok(None) => {
                self.diag.reconciles_completed += 1;
                self.diag.reconciles_unchanged += 1;
                SyncUpdate::Unchanged
            }
            Err(message) => {
                self.diag.reconcile_failures += 1;
                SyncUpdate::Failed(message)
            }
        };
        let succeeded = !matches!(update, SyncUpdate::Failed(_));
        if succeeded && self.config.watch {
            let structural = ChangeSet::STRUCTURE | ChangeSet::OVERFLOW | ChangeSet::MANUAL;
            let refresh_exclusions =
                trigger.intersects(ChangeSet::IGNORE_RULES | ChangeSet::INDEX | structural);
            if let Some(layout) = self.reconciler.layout(refresh_exclusions) {
                let moved = self
                    .layout
                    .as_ref()
                    .is_none_or(|l| !l.same_locations(&layout));
                if self.watcher.is_none() || moved || self.rebuild {
                    if self.restart_at.is_none() {
                        self.start_watcher(layout);
                    }
                } else {
                    self.layout = Some(layout);
                }
            }
        }
        self.diag.git_calls += thread_invocations() - calls;
        self.emit(update);
    }

    fn start_watcher(&mut self, layout: WatchLayout) {
        let had_watcher = self.watcher.take().is_some();
        self.layout = Some(layout.clone());
        match watcher::start(&layout, self.tx.clone(), Arc::clone(&self.dropped)) {
            Ok(w) => {
                let roots = w.watched.len();
                self.watcher = Some(w);
                self.rebuild = false;
                self.restart_failures = 0;
                self.restart_at = None;
                if had_watcher {
                    self.diag.watcher_restarts += 1;
                }
                self.emit(SyncUpdate::Watching { roots });
            }
            Err(e) => {
                self.diag.watcher_errors += 1;
                self.restart_failures += 1;
                let attempt = self.restart_failures as usize;
                self.restart_at =
                    (self.restart_failures <= self.config.max_watch_restarts).then(|| {
                        Instant::now()
                            + RESTART_BACKOFF[(attempt - 1).min(RESTART_BACKOFF.len() - 1)]
                    });
                self.emit(SyncUpdate::WatcherUnavailable(format!(
                    "filesystem watcher failed: {e}"
                )));
            }
        }
    }
}
