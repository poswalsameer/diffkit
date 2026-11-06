//! Application state and its updates. This is the only part of the TUI that
//! calls the Git and diff engines; rendering reads this state only.
//!
//! Loading is lazy: changing the selection marks the diff as pending, and
//! the event loop calls [`App::load_pending`] once queued input has been
//! handled. Loaded diffs (and load failures) are kept in a byte-budgeted
//! LRU cache ([`DiffCache`]) keyed by comparison and file identity.
//!
//! Repository state changes only through [`App::apply_reconciled`], which
//! replaces the snapshot and all four file lists at once. It is used by
//! startup, by manual refresh, and by background synchronization
//! ([`App::apply_sync`]), so there is one refresh pipeline. After each
//! reconciliation, cached diffs are revalidated against Git's fresh
//! metadata and only those whose file changed are dropped.
//!
//! The history screen ([`HistoryState`], key `L`) lists recent local
//! commits and inspects one at a time. It has its own selection, scroll,
//! and diff cache; the working-tree state underneath keeps synchronizing
//! and is shown again unchanged when the screen is closed.

use super::{
    actions::Action,
    diff_view::DiffView,
    layout::{self, DEFAULT_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, SIDEBAR_STEP},
};
use crate::{
    diff::{DiffComparison, DiffEngine, DiffError, DiffFile, DiffOptions, StaleReason},
    git::{
        errors::GitError,
        history::{self, CommitInfo, History},
        models::{FileStatus, RepositorySnapshot},
    },
    sync::{
        cache::{CacheKey, CacheStats, DiffCache, DEFAULT_BUDGET_BYTES},
        reconcile::{self, ListResult, Reconciled},
        ChangeSet, SyncDiagnostics, SyncEvent, SyncUpdate,
    },
};
use ratatui::layout::Rect;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Approximate cache size charged for a load failure.
const FAILURE_BYTES: usize = 512;
/// Columns moved by one horizontal scroll step.
pub const HORIZONTAL_STEP: usize = 8;
/// Diff cache budget of the commit being inspected.
const COMMIT_CACHE_BYTES: usize = 16 << 20;

/// Identity of a file across refreshes: its old and new paths.
pub type FileKey = (Option<PathBuf>, Option<PathBuf>);

fn key_of(f: &DiffFile) -> FileKey {
    (f.old_path.clone(), f.new_path.clone())
}

fn mode_index(c: DiffComparison) -> usize {
    DiffComparison::ALL
        .iter()
        .position(|&m| m == c)
        .expect("known comparison")
}

/// The file list of one comparison and its sidebar position.
#[derive(Debug)]
pub struct ModeState {
    /// The discovered files (shared with the synchronization worker), or
    /// why discovery failed.
    pub list: ListResult,
    pub selected: usize,
    /// First visible sidebar row.
    pub offset: usize,
}

impl ModeState {
    pub fn files(&self) -> &[DiffFile] {
        match &self.list {
            Ok(l) => &l.files,
            Err(_) => &[],
        }
    }

    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.files().get(self.selected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The file changed after discovery; refresh to see it.
    Stale,
    /// The file no longer exists.
    Vanished,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadFailure {
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Debug)]
pub enum Loaded {
    View(Box<DiffView>),
    Failed(LoadFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffScroll {
    /// First visible view row.
    pub top: usize,
    /// First visible content column.
    pub left: usize,
    /// Hunk selected by hunk navigation (or containing `top`).
    pub hunk: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sidebar {
    pub width: u16,
    pub visible: bool,
}

/// One commit opened from the history list: its changed files (against
/// the first parent) and the diff of the selected one.
#[derive(Debug)]
pub struct CommitView {
    pub commit: CommitInfo,
    pub list: ListResult,
    pub selected: usize,
    pub offset: usize,
    scroll: DiffScroll,
    cache: DiffCache<Loaded>,
    pending: bool,
}

impl CommitView {
    pub fn files(&self) -> &[DiffFile] {
        match &self.list {
            Ok(l) => &l.files,
            Err(_) => &[],
        }
    }

    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.files().get(self.selected)
    }

    pub fn loaded(&self) -> Option<&Loaded> {
        self.cache.peek(&CacheKey::of(self.selected_file()?))
    }
}

/// The history screen: recent commits reachable from HEAD (local history
/// only), and optionally one opened commit.
#[derive(Debug)]
pub struct HistoryState {
    /// The listing, or why it could not be read.
    pub commits: Result<History, String>,
    pub selected: usize,
    pub offset: usize,
    pub open: Option<CommitView>,
}

impl HistoryState {
    pub fn list(&self) -> &[CommitInfo] {
        match &self.commits {
            Ok(h) => &h.commits,
            Err(_) => &[],
        }
    }

    pub fn selected_commit(&self) -> Option<&CommitInfo> {
        self.list().get(self.selected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    /// Synchronization and cache counters.
    Diagnostics,
    Error {
        title: String,
        message: String,
    },
}

/// What the synchronization indicator shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncPhase {
    /// No background synchronization: changes appear on `r`.
    Manual,
    Synced,
    ChangesDetected,
    Synchronizing,
    /// The last reconciliation failed; the shown state may be outdated.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    pub phase: SyncPhase,
    /// `Some(false)` with a reason when automatic synchronization is not
    /// running (watcher failed or `--no-watch`); `None` until known.
    pub watching: Option<bool>,
    pub watcher_message: Option<String>,
    pub diagnostics: SyncDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub kind: StatusKind,
    pub text: String,
}

#[derive(Debug)]
pub struct App {
    start: PathBuf,
    options: DiffOptions,
    engine: DiffEngine,
    snapshot: RepositorySnapshot,
    mode: DiffComparison,
    modes: Vec<ModeState>,
    cache: DiffCache<Loaded>,
    pending_load: bool,
    sync: SyncState,
    /// Refreshes go to the background worker instead of running inline.
    background: bool,
    refresh_requested: bool,
    /// A manual refresh is in flight; report its outcome.
    manual_pending: bool,
    /// Scroll position of the working-tree diff.
    scroll: DiffScroll,
    history: Option<HistoryState>,
    sidebar: Sidebar,
    overlay: Option<Overlay>,
    status: Option<Status>,
    size: Rect,
    quit: bool,
}

/// Errors are shown in one line of the interface; keep them short.
fn short(message: &str) -> String {
    let one_line: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    crate::display::truncate_end(one_line.trim(), 300)
}

impl App {
    /// Inspects the repository containing `start`, discovers every
    /// comparison (metadata only), and selects the first file. No diff is
    /// loaded until [`App::load_pending`].
    pub fn open(start: &Path, size: Rect) -> Result<Self, GitError> {
        let options = DiffOptions::default();
        let r = reconcile::reconcile(start, options, ChangeSet::STARTUP, None)?;
        let engine = DiffEngine::from_snapshot(&r.snapshot, options);
        let mut app = Self {
            start: start.to_path_buf(),
            options,
            engine,
            snapshot: r.snapshot,
            mode: DiffComparison::Unstaged,
            modes: modes_of(r.lists),
            cache: DiffCache::new(DEFAULT_BUDGET_BYTES),
            pending_load: true,
            sync: SyncState {
                phase: SyncPhase::Manual,
                watching: None,
                watcher_message: None,
                diagnostics: SyncDiagnostics::default(),
            },
            background: false,
            refresh_requested: false,
            manual_pending: false,
            scroll: DiffScroll::default(),
            history: None,
            sidebar: Sidebar {
                width: DEFAULT_SIDEBAR_WIDTH,
                visible: true,
            },
            overlay: None,
            status: None,
            size,
            quit: false,
        };
        // Open on the first comparison with changes, preferring what the
        // user most likely wants to review.
        app.mode = [
            DiffComparison::Unstaged,
            DiffComparison::Staged,
            DiffComparison::Untracked,
            DiffComparison::Combined,
        ]
        .into_iter()
        .find(|&c| !app.mode_state(c).files().is_empty())
        .unwrap_or(DiffComparison::Unstaged);
        app.keep_selection_visible();
        Ok(app)
    }

    /// The current repository state in the form the synchronization
    /// worker starts from.
    pub fn reconciled(&self) -> Reconciled {
        Reconciled {
            snapshot: self.snapshot.clone(),
            lists: self.modes.iter().map(|m| m.list.clone()).collect(),
            trigger: ChangeSet::STARTUP,
            duration: std::time::Duration::ZERO,
            reused: 0,
            retries: 0,
            settled: true,
        }
    }

    pub fn start_path(&self) -> &Path {
        &self.start
    }

    pub fn options(&self) -> DiffOptions {
        self.options
    }

    // ------------------------------------------------------------ accessors

    pub fn snapshot(&self) -> &RepositorySnapshot {
        &self.snapshot
    }

    pub fn mode(&self) -> DiffComparison {
        self.mode
    }

    pub fn mode_state(&self, c: DiffComparison) -> &ModeState {
        &self.modes[mode_index(c)]
    }

    fn current_mode(&self) -> &ModeState {
        self.mode_state(self.mode)
    }

    fn current_mode_mut(&mut self) -> &mut ModeState {
        let i = mode_index(self.mode);
        &mut self.modes[i]
    }

    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.current_mode().selected_file()
    }

    /// The history screen, when it is open.
    pub fn history(&self) -> Option<&HistoryState> {
        self.history.as_ref()
    }

    fn commit_view(&self) -> Option<&CommitView> {
        self.history.as_ref()?.open.as_ref()
    }

    fn commit_view_mut(&mut self) -> Option<&mut CommitView> {
        self.history.as_mut()?.open.as_mut()
    }

    /// The file whose diff is on screen: the selected file of the opened
    /// commit, or of the current comparison.
    pub fn active_file(&self) -> Option<&DiffFile> {
        match &self.history {
            Some(h) => h.open.as_ref()?.selected_file(),
            None => self.selected_file(),
        }
    }

    /// The loaded diff (or failure) of the file on screen, if loaded.
    pub fn loaded(&self) -> Option<&Loaded> {
        match &self.history {
            Some(h) => h.open.as_ref()?.loaded(),
            None => self.worktree_loaded(),
        }
    }

    fn worktree_loaded(&self) -> Option<&Loaded> {
        self.cache.peek(&CacheKey::of(self.selected_file()?))
    }

    pub fn view(&self) -> Option<&DiffView> {
        match self.loaded()? {
            Loaded::View(v) => Some(v),
            Loaded::Failed(_) => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        match &self.history {
            Some(h) => h
                .open
                .as_ref()
                .is_some_and(|c| c.pending && c.selected_file().is_some() && c.loaded().is_none()),
            None => {
                self.pending_load
                    && self.selected_file().is_some()
                    && self.worktree_loaded().is_none()
            }
        }
    }

    /// Scroll position of the diff on screen.
    pub fn scroll(&self) -> DiffScroll {
        self.commit_view().map_or(self.scroll, |c| c.scroll)
    }

    fn scroll_mut(&mut self) -> &mut DiffScroll {
        match self.history.as_mut().and_then(|h| h.open.as_mut()) {
            Some(c) => &mut c.scroll,
            None => &mut self.scroll,
        }
    }

    /// Column titles of the diff on screen.
    pub fn side_labels(&self) -> (String, String) {
        match self.commit_view() {
            Some(c) => {
                let old = match (c.commit.first_parent(), c.commit.is_merge()) {
                    (Some(p), true) => format!("OLD · first parent {}", &p.as_str()[..7]),
                    (Some(p), false) => format!("OLD · parent {}", &p.as_str()[..7]),
                    (None, _) => "OLD · (empty tree)".into(),
                };
                (old, format!("NEW · commit {}", &c.commit.id.as_str()[..7]))
            }
            None => {
                let (o, n) = super::ui::theme::side_labels(self.mode);
                (o.into(), n.into())
            }
        }
    }

    /// Git's status entry for a path in the working tree (conflict kind,
    /// submodule state), from the current snapshot.
    pub fn status_of(&self, path: &Path) -> Option<&FileStatus> {
        self.snapshot.status.files.iter().find(|f| f.path == path)
    }

    pub fn sidebar(&self) -> Sidebar {
        self.sidebar
    }

    pub fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn size(&self) -> Rect {
        self.size
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn cached_entries(&self) -> usize {
        self.cache.len()
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Sets the diff cache budget (default 64 MiB), evicting as needed.
    pub fn set_cache_budget(&mut self, bytes: usize) {
        self.cache.set_budget(bytes);
    }

    pub fn sync_state(&self) -> &SyncState {
        &self.sync
    }

    /// Switches refreshes to the background worker (the interactive loop
    /// does this once its worker runs).
    pub fn set_background_sync(&mut self, on: bool) {
        self.background = on;
        if on && self.sync.phase == SyncPhase::Manual {
            self.sync.phase = SyncPhase::Synchronizing;
        }
    }

    /// Takes a pending manual refresh request for the background worker.
    pub fn take_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.refresh_requested)
    }

    pub fn screen(&self) -> Option<layout::ScreenLayout> {
        layout::screen(self.size, self.sidebar.width, self.sidebar.visible)
    }

    /// Visible diff rows and content width, when side-by-side fits.
    fn diff_viewport(&self) -> Option<(usize, usize)> {
        let screen = self.screen()?;
        let width = self.view().map_or(3, DiffView::number_width);
        let areas = layout::diff_areas(screen.diff, width).ok()?;
        Some((areas.rows.height as usize, areas.old_text.width as usize))
    }

    fn sidebar_rows(&self) -> usize {
        self.screen()
            .and_then(|s| s.sidebar)
            .map_or(1, layout::sidebar_rows)
            .max(1)
    }

    // -------------------------------------------------------------- updates

    /// Applies an action and loads the selected diff if needed.
    pub fn dispatch(&mut self, action: Action) {
        self.apply(action);
        self.load_pending();
    }

    /// Applies an action without loading. Never runs Git, except for
    /// [`Action::Refresh`] and [`Action::OpenSelected`], which load on
    /// purpose.
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::ToggleHelp => {
                self.overlay = match self.overlay {
                    Some(Overlay::Help) => None,
                    _ => Some(Overlay::Help),
                }
            }
            Action::ToggleDiagnostics => {
                self.overlay = match self.overlay {
                    Some(Overlay::Diagnostics) => None,
                    _ => Some(Overlay::Diagnostics),
                }
            }
            Action::CloseOverlay => {
                if self.overlay.take().is_none() {
                    if self.history.is_some() {
                        self.history_back();
                    } else {
                        self.status = None;
                    }
                }
            }
            Action::ToggleHistory => {
                if self.history.is_some() {
                    self.close_history();
                } else {
                    self.open_history();
                }
            }
            Action::Resize(w, h) => {
                self.size = Rect::new(0, 0, w, h);
                self.clamp_scroll();
                self.keep_selection_visible();
                self.keep_history_visible();
            }
            Action::SelectNextFile => self.move_selection(1),
            Action::SelectPreviousFile => self.move_selection(-1),
            Action::SelectFirstFile => self.select(0),
            Action::SelectLastFile => self.select(usize::MAX),
            Action::SidebarPageDown => self.move_selection(self.sidebar_rows() as isize),
            Action::SidebarPageUp => self.move_selection(-(self.sidebar_rows() as isize)),
            Action::OpenSelected => match &self.history {
                Some(h) if h.open.is_none() => self.open_commit(),
                Some(_) => self.reload_commit_file(),
                None => self.reload_selected(),
            },
            Action::SetComparison(c) => {
                self.close_history();
                self.set_mode(c)
            }
            Action::NextComparison => {
                self.close_history();
                self.set_mode(self.cycle_mode(1))
            }
            Action::PreviousComparison => {
                self.close_history();
                self.set_mode(self.cycle_mode(DiffComparison::ALL.len() - 1))
            }
            Action::ScrollDown(n) => self.scroll_by(n as isize),
            Action::ScrollUp(n) => self.scroll_by(-(n as isize)),
            Action::PageDown => {
                let page = self
                    .diff_viewport()
                    .map_or(1, |v| v.0.saturating_sub(1).max(1));
                self.scroll_by(page as isize);
            }
            Action::PageUp => {
                let page = self
                    .diff_viewport()
                    .map_or(1, |v| v.0.saturating_sub(1).max(1));
                self.scroll_by(-(page as isize));
            }
            Action::ScrollTop => self.scroll_to(0),
            Action::ScrollBottom => self.scroll_to(usize::MAX),
            Action::ScrollLeft => {
                let s = self.scroll_mut();
                s.left = s.left.saturating_sub(HORIZONTAL_STEP)
            }
            Action::ScrollRight => {
                self.scroll_mut().left += HORIZONTAL_STEP;
                self.clamp_scroll();
            }
            Action::ScrollLineStart => self.scroll_mut().left = 0,
            Action::NextHunk => self.goto_hunk(true),
            Action::PreviousHunk => self.goto_hunk(false),
            Action::ToggleSidebar => {
                self.sidebar.visible = !self.sidebar.visible;
                self.clamp_scroll();
            }
            Action::GrowSidebar => self.resize_sidebar(SIDEBAR_STEP as i32),
            Action::ShrinkSidebar => self.resize_sidebar(-(SIDEBAR_STEP as i32)),
            Action::Refresh => {
                if self.history.is_some() {
                    self.reload_history();
                } else {
                    self.refresh()
                }
            }
        }
    }

    // -------------------------------------------------------------- history

    fn read_history(&self) -> Result<History, String> {
        if self.snapshot.branch.unborn() {
            return Ok(History {
                commits: Vec::new(),
                truncated: false,
            });
        }
        let r = &self.snapshot.repository;
        history::recent(
            &r.root,
            &r.common_git_dir.join("shallow"),
            history::DEFAULT_LIMIT,
        )
        .map_err(|e| short(&e.to_string()))
    }

    /// Opens the history screen (one `git log`). The working-tree view is
    /// kept as it is underneath.
    pub fn open_history(&mut self) {
        let commits = self.read_history();
        self.history = Some(HistoryState {
            commits,
            selected: 0,
            offset: 0,
            open: None,
        });
    }

    /// Closes the history screen and shows the working tree as it is now.
    pub fn close_history(&mut self) {
        if self.history.take().is_some() {
            self.pending_load = true;
            self.clamp_scroll();
            self.keep_selection_visible();
        }
    }

    /// Esc: from a commit back to the list, from the list to the working tree.
    fn history_back(&mut self) {
        match &mut self.history {
            Some(h) if h.open.is_some() => {
                h.open = None;
                self.keep_history_visible();
            }
            Some(_) => self.close_history(),
            None => {}
        }
    }

    /// `r` on the history screen: reads the list again, keeping the
    /// selected commit when it is still listed.
    fn reload_history(&mut self) {
        let commits = self.read_history();
        let Some(h) = &mut self.history else { return };
        let previous = h.selected_commit().map(|c| c.id.clone());
        h.commits = commits;
        let list = h.list();
        h.selected = previous
            .and_then(|id| list.iter().position(|c| c.id == id))
            .unwrap_or(0);
        self.keep_history_visible();
        self.status = Some(Status {
            kind: StatusKind::Info,
            text: format!("History reloaded: {} commits", self.history_len()),
        });
    }

    fn history_len(&self) -> usize {
        self.history.as_ref().map_or(0, |h| h.list().len())
    }

    /// Lists the selected commit's files (one `git diff-tree`) and selects
    /// the first; its diff loads on the next [`App::load_pending`].
    fn open_commit(&mut self) {
        let Some(commit) = self
            .history
            .as_ref()
            .and_then(HistoryState::selected_commit)
            .cloned()
        else {
            return;
        };
        let list = if commit.shallow_boundary && commit.parents.is_empty() {
            Err(
                "The parents of this commit are not in this shallow clone, so the \
                 changes it made cannot be computed."
                    .to_string(),
            )
        } else {
            self.engine
                .discover_commit(&commit.id, commit.first_parent())
                .map(Arc::new)
                .map_err(|e| short(&e.to_string()))
        };
        if let Some(h) = &mut self.history {
            h.open = Some(CommitView {
                commit,
                list,
                selected: 0,
                offset: 0,
                scroll: DiffScroll::default(),
                cache: DiffCache::new(COMMIT_CACHE_BYTES),
                pending: true,
            });
        }
    }

    fn history_move(&mut self, delta: isize) {
        let Some(h) = &mut self.history else { return };
        match &mut h.open {
            Some(c) => {
                let len = c.files().len();
                if len == 0 {
                    return;
                }
                let next = (c.selected as isize + delta).clamp(0, len as isize - 1) as usize;
                if next != c.selected {
                    c.selected = next;
                    c.scroll = DiffScroll::default();
                    c.pending = true;
                }
            }
            None => {
                let len = h.list().len();
                if len == 0 {
                    return;
                }
                h.selected = (h.selected as isize + delta).clamp(0, len as isize - 1) as usize;
            }
        }
        self.keep_history_visible();
    }

    fn keep_history_visible(&mut self) {
        let rows = self.sidebar_rows();
        let Some(h) = &mut self.history else { return };
        let (selected, offset, len) = match &mut h.open {
            Some(c) => {
                let len = c.files().len();
                (c.selected, &mut c.offset, len)
            }
            None => {
                let len = h.list().len();
                (h.selected, &mut h.offset, len)
            }
        };
        if selected < *offset {
            *offset = selected;
        } else if selected >= *offset + rows {
            *offset = selected + 1 - rows;
        }
        *offset = (*offset).min(len.saturating_sub(rows));
    }

    fn reload_commit_file(&mut self) {
        let Some(c) = self.commit_view_mut() else {
            return;
        };
        if let Some(f) = c.selected_file() {
            let key = CacheKey::of(f);
            c.cache.remove(&key);
            c.pending = true;
        }
    }

    /// Loads the opened commit's selected file if it is not cached.
    fn load_commit_pending(&mut self) {
        let engine = &self.engine;
        let Some(c) = self.history.as_mut().and_then(|h| h.open.as_mut()) else {
            return;
        };
        if !std::mem::take(&mut c.pending) {
            return;
        }
        let (Ok(list), selected) = (&c.list, c.selected) else {
            return;
        };
        let Some(file) = list.files.get(selected) else {
            return;
        };
        let key = CacheKey::of(file);
        c.cache.protect(Some(key.clone()));
        if c.cache.lookup(&key).is_none() {
            let version = file.clone();
            let loaded = match engine.load_file_diff(list, file.id) {
                Ok(diff) => Loaded::View(Box::new(DiffView::new(diff))),
                Err(e) => Loaded::Failed(failure(&e)),
            };
            let bytes = match &loaded {
                Loaded::View(v) => v.approx_bytes(),
                Loaded::Failed(_) => FAILURE_BYTES,
            };
            c.cache.insert(key, &version, loaded, bytes);
        }
        self.clamp_scroll();
        let top = self.scroll().top;
        let hunk = self.view().and_then(|v| v.hunk_at(top));
        self.scroll_mut().hunk = hunk;
    }

    fn cycle_mode(&self, step: usize) -> DiffComparison {
        let all = DiffComparison::ALL;
        all[(mode_index(self.mode) + step) % all.len()]
    }

    fn set_mode(&mut self, c: DiffComparison) {
        if c == self.mode {
            return;
        }
        self.mode = c;
        self.reset_diff_scroll();
        self.keep_selection_visible();
        self.pending_load = true;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.history.is_some() {
            return self.history_move(delta);
        }
        let current = self.current_mode().selected as isize;
        self.select((current + delta).max(0) as usize);
    }

    fn select(&mut self, index: usize) {
        if self.history.is_some() {
            // First or last: move further than any list is long;
            // `history_move` clamps.
            let delta = if index == 0 {
                isize::MIN / 2
            } else {
                isize::MAX / 2
            };
            return self.history_move(delta);
        }
        let len = self.current_mode().files().len();
        if len == 0 {
            return;
        }
        let index = index.min(len - 1);
        if index != self.current_mode().selected {
            self.current_mode_mut().selected = index;
            self.reset_diff_scroll();
            self.pending_load = true;
        }
        self.keep_selection_visible();
    }

    fn keep_selection_visible(&mut self) {
        let rows = self.sidebar_rows();
        let m = self.current_mode_mut();
        if m.selected < m.offset {
            m.offset = m.selected;
        } else if m.selected >= m.offset + rows {
            m.offset = m.selected + 1 - rows;
        }
        let max_offset = m.files().len().saturating_sub(rows);
        m.offset = m.offset.min(max_offset);
    }

    fn resize_sidebar(&mut self, delta: i32) {
        let total = self.size.width.max(MIN_SIDEBAR_WIDTH);
        let current = layout::effective_sidebar_width(total, self.sidebar.width) as i32;
        let next = (current + delta).max(MIN_SIDEBAR_WIDTH as i32) as u16;
        self.sidebar.width = layout::effective_sidebar_width(total, next);
        self.sidebar.visible = true;
        self.clamp_scroll();
        self.keep_selection_visible();
    }

    fn reset_diff_scroll(&mut self) {
        self.scroll = DiffScroll::default();
    }

    fn max_top(&self) -> usize {
        let rows = self.view().map_or(0, DiffView::row_count);
        let visible = self.diff_viewport().map_or(rows, |v| v.0);
        rows.saturating_sub(visible)
    }

    /// Clamps the scroll position of the diff on screen.
    fn clamp_scroll(&mut self) {
        // Without a loaded diff there is nothing to clamp against; keep the
        // position (a file being reloaded after a change keeps its place)
        // and clamp once it is loaded.
        if self.view().is_none() {
            return;
        }
        let max_top = self.max_top();
        let max_left = match (self.view(), self.diff_viewport()) {
            (Some(v), Some((_, width))) => v.max_width().saturating_sub(width),
            _ => 0,
        };
        let s = self.scroll_mut();
        s.top = s.top.min(max_top);
        s.left = s.left.min(max_left);
    }

    fn scroll_by(&mut self, delta: isize) {
        let top = (self.scroll().top as isize + delta).max(0) as usize;
        self.scroll_to(top);
    }

    fn scroll_to(&mut self, top: usize) {
        self.scroll_mut().top = top;
        self.clamp_scroll();
        let top = self.scroll().top;
        let hunk = self.view().and_then(|v| v.hunk_at(top));
        self.scroll_mut().hunk = hunk;
    }

    fn goto_hunk(&mut self, forward: bool) {
        let Some(view) = self.view() else { return };
        let starts = view.hunk_starts().to_vec();
        if starts.is_empty() {
            return;
        }
        let scroll = self.scroll();
        let current = scroll.hunk.or_else(|| view.hunk_at(scroll.top));
        let target = match (current, forward) {
            (None, true) => 0,
            (None, false) => return,
            (Some(i), true) => (i + 1).min(starts.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.scroll_mut().top = starts[target];
        self.clamp_scroll();
        self.scroll_mut().hunk = Some(target);
    }

    // -------------------------------------------------------------- loading

    /// Loads the diff on screen if it is not cached. The only place,
    /// besides refresh and opening history, where the TUI runs Git.
    pub fn load_pending(&mut self) {
        if self.history.is_some() {
            // The working tree's pending load waits until history closes.
            return self.load_commit_pending();
        }
        if !self.pending_load {
            return;
        }
        self.pending_load = false;
        let Some(file) = self.selected_file() else {
            self.cache.protect(None);
            return;
        };
        let key = CacheKey::of(file);
        self.cache.protect(Some(key.clone()));
        if self.cache.lookup(&key).is_some() {
            self.clamp_scroll();
            return;
        }
        self.load_selected();
    }

    fn reload_selected(&mut self) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let key = CacheKey::of(file);
        self.cache.remove(&key);
        self.cache.protect(Some(key));
        self.load_selected();
    }

    fn load_selected(&mut self) {
        let (Ok(list), selected) = (&self.current_mode().list, self.current_mode().selected) else {
            return;
        };
        let Some(file) = list.files.get(selected) else {
            return;
        };
        let key = CacheKey::of(file);
        let version = file.clone();
        let loaded = match self.engine.load_file_diff(list, file.id) {
            Ok(diff) => Loaded::View(Box::new(DiffView::new(diff))),
            Err(e) => Loaded::Failed(failure(&e)),
        };
        let bytes = match &loaded {
            Loaded::View(v) => v.approx_bytes(),
            Loaded::Failed(_) => FAILURE_BYTES,
        };
        if let Loaded::Failed(f) = &loaded {
            self.status = Some(Status {
                kind: if f.kind == FailureKind::Other {
                    StatusKind::Error
                } else {
                    StatusKind::Warning
                },
                text: f.message.clone(),
            });
        }
        self.cache.insert(key, &version, loaded, bytes);
        self.clamp_scroll();
        self.scroll.hunk = self.view().and_then(|v| v.hunk_at(self.scroll.top));
    }

    /// Manual refresh (`r`). With background synchronization the request is
    /// handed to the worker; otherwise it reconciles here. Both paths end in
    /// [`App::apply_reconciled`].
    pub fn refresh(&mut self) {
        self.manual_pending = true;
        if self.background {
            self.refresh_requested = true;
            self.sync.phase = SyncPhase::Synchronizing;
            return;
        }
        match reconcile::reconcile(&self.start, self.options, ChangeSet::MANUAL, None) {
            Ok(r) if !r.settled => self.report_manual(Some(
                "Git state kept changing while it was read; press r again".into(),
            )),
            Ok(r) => {
                self.apply_reconciled(r);
                self.report_manual(None);
            }
            Err(e) => self.report_manual(Some(e.to_string())),
        }
    }

    fn report_manual(&mut self, error: Option<String>) {
        if !std::mem::take(&mut self.manual_pending) {
            return;
        }
        match error {
            None => {
                let count = |c| self.mode_state(c).files().len();
                self.status = Some(Status {
                    kind: StatusKind::Info,
                    text: format!(
                        "Refreshed: {} staged, {} unstaged, {} combined, {} untracked",
                        count(DiffComparison::Staged),
                        count(DiffComparison::Unstaged),
                        count(DiffComparison::Combined),
                        count(DiffComparison::Untracked)
                    ),
                });
            }
            Some(e) => {
                let message = short(&e);
                self.status = Some(Status {
                    kind: StatusKind::Error,
                    text: format!("Refresh failed: {message}"),
                });
                self.overlay = Some(Overlay::Error {
                    title: "Refresh failed".into(),
                    message,
                });
            }
        }
    }

    /// Publishes a reconciliation result: the snapshot and all four lists
    /// are replaced together. Each comparison keeps its selected file when
    /// it is still listed (by old and new path, then by path, else the
    /// nearest position). Cached diffs are kept only when Git reports
    /// identical metadata for their file.
    pub fn apply_reconciled(&mut self, r: Reconciled) {
        let previous: Vec<(Option<FileKey>, usize)> = self
            .modes
            .iter()
            .map(|m| (m.selected_file().map(key_of), m.selected))
            .collect();
        let viewed = self
            .selected_file()
            .map(|f| (key_of(f), f.path().to_path_buf()));
        self.engine = DiffEngine::from_snapshot(&r.snapshot, self.options);
        self.snapshot = r.snapshot;
        self.modes = modes_of(r.lists);
        for (m, (key, index)) in self.modes.iter_mut().zip(previous) {
            let files = m.files();
            m.selected = key
                .and_then(|k| {
                    files.iter().position(|f| key_of(f) == k).or_else(|| {
                        files
                            .iter()
                            .position(|f| Some(f.path()) == k.1.as_deref().or(k.0.as_deref()))
                    })
                })
                .unwrap_or(index.min(files.len().saturating_sub(1)));
        }
        self.revalidate_cache();
        let now = self.selected_file().map(key_of);
        if let Some((key, path)) = viewed {
            if now.as_ref() != Some(&key) {
                self.reset_diff_scroll();
                self.status = Some(Status {
                    kind: StatusKind::Info,
                    text: format!(
                        "{} is no longer in {}",
                        crate::display::display_path(&path),
                        super::ui::theme::mode_title(self.mode)
                    ),
                });
            }
        }
        self.cache.protect(self.selected_file().map(CacheKey::of));
        self.keep_selection_visible();
        self.pending_load = true;
        if self.history.is_none() {
            self.clamp_scroll();
        }
    }

    fn revalidate_cache(&mut self) {
        if self.cache.is_empty() {
            return;
        }
        let mut index: HashMap<(DiffComparison, Option<&Path>, Option<&Path>), &DiffFile> =
            HashMap::new();
        for f in self.modes.iter().flat_map(|m| m.files()) {
            index.insert(
                (f.comparison, f.old_path.as_deref(), f.new_path.as_deref()),
                f,
            );
        }
        self.cache.revalidate(|k| {
            index
                .get(&(k.comparison, k.old_path.as_deref(), k.new_path.as_deref()))
                .copied()
        });
    }

    /// Applies an update from the synchronization worker.
    pub fn apply_sync(&mut self, event: SyncEvent) {
        self.sync.diagnostics = event.diagnostics;
        let settled = |pending: bool| {
            if pending {
                SyncPhase::ChangesDetected
            } else {
                SyncPhase::Synced
            }
        };
        match event.update {
            SyncUpdate::Watching { .. } => {
                self.sync.watching = Some(true);
                self.sync.watcher_message = None;
            }
            SyncUpdate::WatcherUnavailable(message) => {
                let first = self.sync.watching != Some(false);
                self.sync.watching = Some(false);
                if first {
                    self.status = Some(Status {
                        kind: StatusKind::Warning,
                        text: format!("{}. Press r to refresh manually.", short(&message)),
                    });
                }
                self.sync.watcher_message = Some(message);
                if self.sync.phase == SyncPhase::Synchronizing && !self.manual_pending {
                    self.sync.phase = SyncPhase::Synced;
                }
            }
            SyncUpdate::ChangesDetected => {
                if self.sync.phase != SyncPhase::Synchronizing {
                    self.sync.phase = SyncPhase::ChangesDetected;
                }
            }
            SyncUpdate::Synchronizing => self.sync.phase = SyncPhase::Synchronizing,
            SyncUpdate::Reconciled(r) => {
                self.apply_reconciled(*r);
                self.sync.phase = settled(event.pending);
                self.report_manual(None);
            }
            SyncUpdate::Unchanged => {
                self.sync.phase = settled(event.pending);
                self.report_manual(None);
            }
            SyncUpdate::Failed(message) => {
                self.sync.phase = SyncPhase::Failed(short(&message));
                if self.manual_pending {
                    self.report_manual(Some(message));
                } else {
                    self.status = Some(Status {
                        kind: StatusKind::Error,
                        text: format!("Sync failed: {}", short(&message)),
                    });
                }
            }
        }
    }
}

fn modes_of(lists: Vec<ListResult>) -> Vec<ModeState> {
    lists
        .into_iter()
        .map(|list| ModeState {
            list,
            selected: 0,
            offset: 0,
        })
        .collect()
}

fn failure(e: &DiffError) -> LoadFailure {
    match e {
        DiffError::Stale { path, reason } => LoadFailure {
            kind: FailureKind::Stale,
            message: short(&format!(
                "{} changed after the file list was loaded ({}). Press r to refresh.",
                crate::display::display_path(path),
                match reason {
                    StaleReason::MissingFromDiff => "Git no longer reports this change",
                    StaleReason::ObjectChanged => "the staged content changed",
                    StaleReason::StatsChanged => "the line counts changed",
                    StaleReason::BinaryChanged => "its binary status changed",
                    StaleReason::WorkingTreeChanged => "the working-tree file changed",
                }
            )),
        },
        DiffError::Vanished { path } => LoadFailure {
            kind: FailureKind::Vanished,
            message: short(&format!(
                "{} no longer exists. Press r to refresh.",
                crate::display::display_path(path)
            )),
        },
        other => LoadFailure {
            kind: FailureKind::Other,
            message: short(&other.to_string()),
        },
    }
}
