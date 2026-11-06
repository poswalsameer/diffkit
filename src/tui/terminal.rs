//! Terminal lifecycle and the event loop.
//!
//! [`TerminalGuard`] enables raw mode and the alternate screen and restores
//! both, plus cursor visibility, when dropped: on normal exit, on any error
//! returned from the loop, and during unwinding. A panic hook additionally
//! restores the terminal before the panic message is printed, so the
//! message is readable and the shell is usable afterwards. Mouse capture is
//! never enabled, so the terminal's own text selection keeps working.

use super::{actions::Action, app::App, keymap, ui};
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
    sync::Once,
    time::Duration,
};

#[derive(Debug)]
pub enum TuiError {
    /// stdin or stdout is not a terminal.
    NoTerminal,
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
pub fn run(start: &Path) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::NoTerminal);
    }
    let (w, h) = terminal::size()?;
    let mut app = App::open(start, Rect::new(0, 0, w, h))?;
    let guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let result = event_loop(&mut terminal, &mut app);
    drop(terminal);
    drop(guard);
    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<(), TuiError>
where
    TuiError: From<B::Error>,
{
    loop {
        if app.is_loading() {
            // Show "Loading…" before a potentially slow diff load.
            terminal.draw(|f| ui::render(f, app))?;
            app.load_pending();
        }
        terminal.draw(|f| ui::render(f, app))?;
        handle(app, event::read()?);
        // Apply all queued input before loading, so holding j does not load
        // every file it passes.
        while !app.should_quit() && event::poll(Duration::ZERO)? {
            handle(app, event::read()?);
        }
        if app.should_quit() {
            return Ok(());
        }
        app.load_pending();
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
