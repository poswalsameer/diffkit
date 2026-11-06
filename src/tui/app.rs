//! Application state and its updates. This is the only part of the TUI that
//! calls the Git and diff engines; rendering reads this state only.
//!
//! Loading is lazy: changing the selection marks the diff as pending, and
//! the event loop calls [`App::load_pending`] once queued input has been
//! handled. Loaded diffs (and load failures) are kept in a small bounded
//! cache keyed by comparison and file, cleared on refresh.

use super::{
    actions::Action,
    diff_view::DiffView,
    layout::{self, DEFAULT_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, SIDEBAR_STEP},
};
use crate::{
    diff::{
        DiffComparison, DiffEngine, DiffError, DiffFile, DiffFileList, DiffOptions, StaleReason,
    },
    git::{errors::GitError, models::RepositorySnapshot, snapshot},
};
use ratatui::layout::Rect;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

/// Loaded diffs kept for quick return to recently viewed files.
pub const CACHE_CAPACITY: usize = 16;
/// Columns moved by one horizontal scroll step.
pub const HORIZONTAL_STEP: usize = 8;

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
    /// The discovered files, or why discovery failed.
    pub list: Result<DiffFileList, String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Error { title: String, message: String },
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
    cache: VecDeque<((DiffComparison, FileKey), Loaded)>,
    pending_load: bool,
    scroll: DiffScroll,
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
        let snapshot = snapshot::inspect(start)?;
        let engine = DiffEngine::from_snapshot(&snapshot, options);
        let mut app = Self {
            start: start.to_path_buf(),
            options,
            engine,
            snapshot,
            mode: DiffComparison::Unstaged,
            modes: Vec::new(),
            cache: VecDeque::new(),
            pending_load: true,
            scroll: DiffScroll::default(),
            sidebar: Sidebar {
                width: DEFAULT_SIDEBAR_WIDTH,
                visible: true,
            },
            overlay: None,
            status: None,
            size,
            quit: false,
        };
        app.modes = app.discover_all();
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

    fn discover_all(&self) -> Vec<ModeState> {
        DiffComparison::ALL
            .iter()
            .map(|&c| ModeState {
                list: self.engine.discover(c).map_err(|e| short(&e.to_string())),
                selected: 0,
                offset: 0,
            })
            .collect()
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

    /// The loaded diff (or failure) of the selected file, if loaded.
    pub fn loaded(&self) -> Option<&Loaded> {
        let key = (self.mode, key_of(self.selected_file()?));
        self.cache.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    pub fn view(&self) -> Option<&DiffView> {
        match self.loaded()? {
            Loaded::View(v) => Some(v),
            Loaded::Failed(_) => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        self.pending_load && self.selected_file().is_some() && self.loaded().is_none()
    }

    pub fn scroll(&self) -> DiffScroll {
        self.scroll
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
            Action::CloseOverlay => {
                if self.overlay.take().is_none() {
                    self.status = None;
                }
            }
            Action::Resize(w, h) => {
                self.size = Rect::new(0, 0, w, h);
                self.clamp_scroll();
                self.keep_selection_visible();
            }
            Action::SelectNextFile => self.move_selection(1),
            Action::SelectPreviousFile => self.move_selection(-1),
            Action::SelectFirstFile => self.select(0),
            Action::SelectLastFile => self.select(usize::MAX),
            Action::SidebarPageDown => self.move_selection(self.sidebar_rows() as isize),
            Action::SidebarPageUp => self.move_selection(-(self.sidebar_rows() as isize)),
            Action::OpenSelected => self.reload_selected(),
            Action::SetComparison(c) => self.set_mode(c),
            Action::NextComparison => self.set_mode(self.cycle_mode(1)),
            Action::PreviousComparison => {
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
                self.scroll.left = self.scroll.left.saturating_sub(HORIZONTAL_STEP)
            }
            Action::ScrollRight => {
                self.scroll.left += HORIZONTAL_STEP;
                self.clamp_scroll();
            }
            Action::ScrollLineStart => self.scroll.left = 0,
            Action::NextHunk => self.goto_hunk(true),
            Action::PreviousHunk => self.goto_hunk(false),
            Action::ToggleSidebar => {
                self.sidebar.visible = !self.sidebar.visible;
                self.clamp_scroll();
            }
            Action::GrowSidebar => self.resize_sidebar(SIDEBAR_STEP as i32),
            Action::ShrinkSidebar => self.resize_sidebar(-(SIDEBAR_STEP as i32)),
            Action::Refresh => self.refresh(),
        }
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
        let current = self.current_mode().selected as isize;
        self.select((current + delta).max(0) as usize);
    }

    fn select(&mut self, index: usize) {
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

    fn clamp_scroll(&mut self) {
        self.scroll.top = self.scroll.top.min(self.max_top());
        let max_left = match (self.view(), self.diff_viewport()) {
            (Some(v), Some((_, width))) => v.max_width().saturating_sub(width),
            _ => 0,
        };
        self.scroll.left = self.scroll.left.min(max_left);
    }

    fn scroll_by(&mut self, delta: isize) {
        let top = (self.scroll.top as isize + delta).max(0) as usize;
        self.scroll_to(top);
    }

    fn scroll_to(&mut self, top: usize) {
        self.scroll.top = top;
        self.clamp_scroll();
        self.scroll.hunk = self.view().and_then(|v| v.hunk_at(self.scroll.top));
    }

    fn goto_hunk(&mut self, forward: bool) {
        let Some(view) = self.view() else { return };
        let starts = view.hunk_starts().to_vec();
        if starts.is_empty() {
            return;
        }
        let current = self.scroll.hunk.or_else(|| view.hunk_at(self.scroll.top));
        let target = match (current, forward) {
            (None, true) => 0,
            (None, false) => return,
            (Some(i), true) => (i + 1).min(starts.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.scroll.top = starts[target];
        self.clamp_scroll();
        self.scroll.hunk = Some(target);
    }

    // -------------------------------------------------------------- loading

    /// Loads the selected file's diff if it is not cached. The only place,
    /// besides refresh, where the TUI runs Git.
    pub fn load_pending(&mut self) {
        if !self.pending_load {
            return;
        }
        self.pending_load = false;
        if self.selected_file().is_none() || self.loaded().is_some() {
            self.clamp_scroll();
            return;
        }
        self.load_selected();
    }

    fn reload_selected(&mut self) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let key = (self.mode, key_of(file));
        self.cache.retain(|(k, _)| *k != key);
        self.load_selected();
    }

    fn load_selected(&mut self) {
        let (Ok(list), selected) = (&self.current_mode().list, self.current_mode().selected) else {
            return;
        };
        let Some(file) = list.files.get(selected) else {
            return;
        };
        let key = (self.mode, key_of(file));
        let loaded = match self.engine.load_file_diff(list, file.id) {
            Ok(diff) => Loaded::View(Box::new(DiffView::new(diff))),
            Err(e) => Loaded::Failed(failure(&e)),
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
        self.cache.retain(|(k, _)| *k != key);
        if self.cache.len() >= CACHE_CAPACITY {
            self.cache.pop_front();
        }
        self.cache.push_back((key, loaded));
        self.clamp_scroll();
        self.scroll.hunk = self.view().and_then(|v| v.hunk_at(self.scroll.top));
    }

    /// Re-inspects the repository and rediscovers every comparison. The
    /// selection of each comparison is kept when its file is still listed.
    pub fn refresh(&mut self) {
        let snapshot = match snapshot::inspect(&self.start) {
            Ok(s) => s,
            Err(e) => {
                let message = short(&e.to_string());
                self.status = Some(Status {
                    kind: StatusKind::Error,
                    text: format!("Refresh failed: {message}"),
                });
                self.overlay = Some(Overlay::Error {
                    title: "Refresh failed".into(),
                    message,
                });
                return;
            }
        };
        let previous: Vec<(Option<FileKey>, usize)> = self
            .modes
            .iter()
            .map(|m| (m.selected_file().map(key_of), m.selected))
            .collect();
        let viewed = self.selected_file().map(key_of);
        self.engine = DiffEngine::from_snapshot(&snapshot, self.options);
        self.snapshot = snapshot;
        self.modes = self.discover_all();
        for (m, (key, index)) in self.modes.iter_mut().zip(previous) {
            let files = match &m.list {
                Ok(l) => &l.files,
                Err(_) => continue,
            };
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
        self.cache.clear();
        if self.selected_file().map(key_of) != viewed {
            self.reset_diff_scroll();
        }
        self.keep_selection_visible();
        self.pending_load = true;
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
