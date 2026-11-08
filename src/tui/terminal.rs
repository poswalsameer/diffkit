//! Terminal lifecycle and the event loop.
//!
//! [`TerminalGuard`] enables raw mode and the alternate screen and restores
//! both, plus cursor visibility, when dropped: on normal exit, on any error
//! returned from the loop, and during unwinding. A panic hook additionally
//! restores the terminal before the panic message is printed, so the
//! message is readable and the shell is usable afterwards. Mouse capture is
//! never enabled, so the terminal's own text selection keeps working.
//!
//! Threads while the interface runs:
//! - main: owns [`App`], renders, and applies input, synchronization
//!   results, and loaded diffs;
//! - input: reads terminal events (checking a stop flag every 200 ms);
//! - loader ([`Loader`]): runs one diff load at a time, so a slow load
//!   never stops the interface from responding (it shows "Loading…");
//! - sync worker ([`SyncService`]): watcher, debounce, reconciliation.
//!
//! Input, sync results, and loaded diffs arrive on one bounded channel, so
//! the main thread sleeps until something happens; there is no polling or
//! frame timer. On exit, the input thread is joined, the loader is told to
//! stop and detached (it may be inside a long Git command; it ends when
//! that returns), the channel is dropped (so neither the loader nor the
//! worker can block sending), and the worker gets a bounded time to stop.
//!
//! On Unix, SIGTERM, SIGHUP, and SIGINT sent from outside (raw mode turns
//! Ctrl+C into a key, so SIGINT only comes from `kill`) end the session the
//! same way as `q`: the input thread notices within 200 ms and the loop
//! returns, so the terminal is restored. A second signal while that is in
//! progress exits immediately.

use super::{
    actions::Action,
    app::{App, LoadJob, LoadResult},
    keymap, ui,
};
use crate::sync::scheduler::{GitReconciler, SyncConfig, SyncEvent, SyncService};
use crossterm::{
    cursor,
    event::{self, Event},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use std::{
    io::{self, IsTerminal},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Condvar, Mutex, Once,
    },
    thread::JoinHandle,
    time::Duration,
};

/// Interactive session options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    /// Watch the filesystem and synchronize automatically.
    pub watch: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { watch: true }
    }
}

/// Everything the main loop reacts to.
enum LoopEvent {
    Input(Event),
    InputFailed(io::Error),
    Sync(Box<SyncEvent>),
    /// A diff finished loading on the loader thread.
    Loaded(Box<LoadResult>),
    /// A termination signal arrived.
    Signal(i32),
}

/// Capacity of the main loop's channel. Producers block when it is full;
/// the main loop drains it completely on every iteration.
const LOOP_QUEUE: usize = 256;

#[derive(Debug)]
pub enum TuiError {
    /// stdin or stdout is not a terminal.
    NoTerminal,
    /// The session ended because of this signal; the terminal was restored.
    Signal(i32),
    Git(crate::git::errors::GitError),
    Io(io::Error),
}

impl std::fmt::Display for TuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTerminal => write!(
                f,
                "the interactive interface needs a terminal (stdin and stdout must be a TTY); \
                 for plain output use `diffkit inspect` or `diffkit diff-stats`"
            ),
            Self::Signal(n) => write!(f, "stopped by signal {n}"),
            Self::Git(e) => e.fmt(f),
            Self::Io(e) => write!(f, "terminal error: {e}"),
        }
    }
}

impl std::error::Error for TuiError {}

impl From<io::Error> for TuiError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<crate::git::errors::GitError> for TuiError {
    fn from(e: crate::git::errors::GitError) -> Self {
        Self::Git(e)
    }
}

/// Puts the terminal back: leaves the alternate screen, shows the cursor,
/// and disables raw mode. Safe to call more than once.
pub fn restore_terminal() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
}

/// Installs (once) a panic hook that restores the terminal and then runs
/// the previously installed hook, so the panic message stays readable.
pub fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}

/// RAII owner of raw mode and the alternate screen.
pub struct TerminalGuard {
    raw: bool,
    alternate: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        let mut guard = Self {
            raw: false,
            alternate: false,
        };
        terminal::enable_raw_mode()?;
        guard.raw = true;
        // A plain clear: unlike `Terminal::clear`, it does not query the
        // cursor position, which some terminals and multiplexers never answer.
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All)
        )?;
        guard.alternate = true;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.alternate {
            let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        }
        if self.raw {
            let _ = terminal::disable_raw_mode();
        }
    }
}

/// Runs the interactive interface for the repository containing `start`.
/// The repository is inspected before the terminal is touched, so errors
/// such as "not a Git repository" print normally.
pub fn run(start: &Path, options: RunOptions) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::NoTerminal);
    }
    let (w, h) = terminal::size()?;
    let mut app = App::open(start, Rect::new(0, 0, w, h))?;
    let (tx, rx) = mpsc::sync_channel::<LoopEvent>(LOOP_QUEUE);
    let config = SyncConfig {
        watch: options.watch,
        ..SyncConfig::default()
    };
    let reconciler = GitReconciler::new(
        app.start_path().to_path_buf(),
        app.options(),
        app.reconciled(),
    );
    let sync_tx = tx.clone();
    let sync = SyncService::start(config, reconciler, move |e| {
        // Fails only after the main loop has ended.
        let _ = sync_tx.send(LoopEvent::Sync(Box::new(e)));
    });
    app.set_background_sync(true);
    let signals = Signals::register()?;
    let guard = TerminalGuard::enter()?;
    let loader = Loader::start(tx.clone());
    let input = InputThread::start(tx, signals.received());
    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(TuiError::from)
        .and_then(|mut terminal| event_loop(&mut terminal, &mut app, &rx, &sync, &loader));
    input.stop();
    loader.stop();
    drop(rx);
    sync.shutdown(Duration::from_secs(2));
    drop(guard);
    drop(signals);
    result
}

/// Termination signals routed to the event loop while the interface runs.
struct Signals {
    /// The last signal received (0: none).
    received: Arc<AtomicUsize>,
    #[cfg(unix)]
    ids: Vec<signal_hook::SigId>,
}

impl Signals {
    #[cfg(unix)]
    fn register() -> io::Result<Self> {
        use signal_hook::{consts, flag};
        let received = Arc::new(AtomicUsize::new(0));
        let any = Arc::new(AtomicBool::new(false));
        let mut ids = Vec::new();
        for sig in [consts::SIGTERM, consts::SIGHUP, consts::SIGINT] {
            // Registered first, so it sees `any` as it was before this
            // signal: only a second signal exits at once.
            ids.push(flag::register_conditional_shutdown(
                sig,
                128 + sig,
                Arc::clone(&any),
            )?);
            ids.push(flag::register(sig, Arc::clone(&any))?);
            ids.push(flag::register_usize(
                sig,
                Arc::clone(&received),
                sig as usize,
            )?);
        }
        Ok(Self { received, ids })
    }

    #[cfg(not(unix))]
    fn register() -> io::Result<Self> {
        Ok(Self {
            received: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn received(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.received)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        #[cfg(unix)]
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: &Receiver<LoopEvent>,
    sync: &SyncService,
    loader: &Loader,
) -> Result<(), TuiError>
where
    TuiError: From<B::Error>,
{
    loop {
        // The diff on screen loads in the background; until it arrives the
        // frame drawn next shows "Loading…" and input keeps working.
        if let Some(job) = app.take_load() {
            loader.submit(job);
        }
        terminal.draw(|f| ui::render(f, app))?;
        let Ok(first) = rx.recv() else { return Ok(()) };
        handle_loop_event(app, first)?;
        // Apply everything queued before loading, so holding j does not
        // load every file it passes and bursts of updates render once.
        while !app.should_quit() {
            match rx.try_recv() {
                Ok(e) => handle_loop_event(app, e)?,
                Err(_) => break,
            }
        }
        if app.should_quit() {
            return Ok(());
        }
        if app.take_refresh_request() {
            sync.request_refresh();
        }
    }
}

fn handle_loop_event(app: &mut App, event: LoopEvent) -> Result<(), TuiError> {
    match event {
        LoopEvent::Input(e) => handle(app, e),
        LoopEvent::Sync(e) => app.apply_sync(*e),
        LoopEvent::Loaded(r) => app.finish_load(*r),
        LoopEvent::InputFailed(e) => return Err(TuiError::Io(e)),
        LoopEvent::Signal(n) => return Err(TuiError::Signal(n)),
    }
    Ok(())
}

/// Reads terminal events on its own thread so the main loop can also wait
/// for synchronization results.
struct InputThread {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl InputThread {
    fn start(tx: SyncSender<LoopEvent>, signal: Arc<AtomicUsize>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("diffkit-input".into())
            .spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    let sig = signal.load(Ordering::SeqCst);
                    if sig != 0 {
                        let _ = tx.send(LoopEvent::Signal(sig as i32));
                        return;
                    }
                    let event = match event::poll(Duration::from_millis(200)) {
                        Ok(false) => continue,
                        Ok(true) => event::read().map(LoopEvent::Input),
                        Err(e) => Err(e),
                    };
                    let failed = event.is_err();
                    let message = event.unwrap_or_else(LoopEvent::InputFailed);
                    if tx.send(message).is_err() || failed {
                        return;
                    }
                }
            })
            .expect("spawn input thread");
        Self { stop, handle }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
    }
}

/// Runs diff loads on its own thread, one at a time. At most one job
/// waits: a newer one replaces it (the selection moved on), so nothing
/// piles up however fast the selection changes. Results go to the main
/// loop's channel.
struct Loader {
    slot: Arc<(Mutex<LoaderSlot>, Condvar)>,
}

#[derive(Default)]
struct LoaderSlot {
    job: Option<LoadJob>,
    stop: bool,
}

impl Loader {
    fn start(tx: SyncSender<LoopEvent>) -> Self {
        let slot = Arc::new((Mutex::new(LoaderSlot::default()), Condvar::new()));
        let shared = Arc::clone(&slot);
        std::thread::Builder::new()
            .name("diffkit-loader".into())
            .spawn(move || loop {
                let job = {
                    let (lock, ready) = &*shared;
                    let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if s.stop {
                            return;
                        }
                        if let Some(job) = s.job.take() {
                            break job;
                        }
                        s = ready.wait(s).unwrap_or_else(|e| e.into_inner());
                    }
                };
                if tx.send(LoopEvent::Loaded(Box::new(job.run()))).is_err() {
                    return;
                }
            })
            .expect("spawn loader thread");
        Self { slot }
    }

    fn submit(&self, job: LoadJob) {
        let (lock, ready) = &*self.slot;
        lock.lock().unwrap_or_else(|e| e.into_inner()).job = Some(job);
        ready.notify_one();
    }

    /// Asks the thread to stop without waiting: a load in progress finishes
    /// its Git command first, and its result is discarded.
    fn stop(self) {
        let (lock, ready) = &*self.slot;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        s.stop = true;
        s.job = None;
        ready.notify_one();
    }
}

fn handle(app: &mut App, event: Event) {
    let action = match event {
        Event::Key(key) => keymap::action_for(key, app.overlay()),
        Event::Resize(w, h) => Some(Action::Resize(w, h)),
        _ => None,
    };
    if let Some(a) = action {
        app.apply(a);
    }
}
