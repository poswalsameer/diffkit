//! Semantic styles. Only the standard 16 colors plus `REVERSED`/`BOLD` are
//! used, and only as foregrounds (no full-line backgrounds), so the
//! terminal's own palette keeps text readable on light and dark themes.
//! Meaning is never carried by color alone: rows also have `-`/`+`/`~`
//! markers and files have status letters.

use crate::types::diff::{DiffComparison, DiffFile, DiffRowKind, FileChangeKind};
use ratatui::style::{Color, Modifier, Style};

pub fn plain() -> Style {
    Style::new()
}

pub fn muted() -> Style {
    Style::new().fg(Color::DarkGray)
}

pub fn added() -> Style {
    Style::new().fg(Color::Green)
}

pub fn removed() -> Style {
    Style::new().fg(Color::Red)
}

pub fn modified() -> Style {
    Style::new().fg(Color::Yellow)
}

pub fn accent() -> Style {
    Style::new().fg(Color::Cyan)
}

pub fn conflict() -> Style {
    Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

pub fn title() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub fn brand() -> Style {
    Style::new()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// Escapes standing in for control characters or invalid bytes.
pub fn escape() -> Style {
    Style::new().fg(Color::Magenta)
}

pub fn warning() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}

pub fn error() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
}

/// Content style for one side of a row.
pub fn side(kind: DiffRowKind, old_side: bool) -> Style {
    match (kind, old_side) {
        (DiffRowKind::Removed, _) => removed(),
        (DiffRowKind::Added, _) => added(),
        (DiffRowKind::Modified, true) => removed(),
        (DiffRowKind::Modified, false) => added(),
        (DiffRowKind::Context | DiffRowKind::Empty, _) => plain(),
    }
}

/// Gutter marker for one side of a row.
pub fn marker(kind: DiffRowKind, old_side: bool) -> (&'static str, Style) {
    match (kind, old_side) {
        (DiffRowKind::Removed, true) => ("-", removed()),
        (DiffRowKind::Added, false) => ("+", added()),
        (DiffRowKind::Modified, _) => ("~", modified()),
        _ => (" ", plain()),
    }
}

/// One- or two-letter status code and its style for the sidebar.
pub fn status_code(f: &DiffFile) -> (&'static str, Style) {
    if f.comparison == DiffComparison::Untracked {
        return ("??", added());
    }
    match f.kind {
        FileChangeKind::Added => ("A", added()),
        FileChangeKind::Modified => ("M", modified()),
        FileChangeKind::Deleted => ("D", removed()),
        FileChangeKind::Renamed => ("R", accent()),
        FileChangeKind::Copied => ("C", accent()),
        FileChangeKind::TypeChanged => ("T", modified()),
        FileChangeKind::ModeChanged => ("X", modified()),
        FileChangeKind::Unmerged => ("U", conflict()),
        FileChangeKind::Unknown(_) => ("?", muted()),
    }
}

pub fn kind_name(f: &DiffFile) -> &'static str {
    if f.comparison == DiffComparison::Untracked {
        return "Untracked";
    }
    match f.kind {
        FileChangeKind::Added => "Added",
        FileChangeKind::Modified => "Modified",
        FileChangeKind::Deleted => "Deleted",
        FileChangeKind::Renamed => "Renamed",
        FileChangeKind::Copied => "Copied",
        FileChangeKind::TypeChanged => "Type changed",
        FileChangeKind::ModeChanged => "Mode changed",
        FileChangeKind::Unmerged => "Unmerged",
        FileChangeKind::Unknown(_) => "Unknown change",
    }
}

pub fn mode_title(c: DiffComparison) -> &'static str {
    match c {
        DiffComparison::Staged => "STAGED",
        DiffComparison::Unstaged => "UNSTAGED",
        DiffComparison::Combined => "COMBINED",
        DiffComparison::Untracked => "UNTRACKED",
        DiffComparison::Commit => "COMMIT",
    }
}

/// Column titles: what each side of the comparison is.
pub fn side_labels(c: DiffComparison) -> (&'static str, &'static str) {
    match c {
        DiffComparison::Staged => ("OLD · HEAD", "NEW · index"),
        DiffComparison::Unstaged => ("OLD · index", "NEW · working tree"),
        DiffComparison::Combined => ("OLD · HEAD", "NEW · working tree"),
        DiffComparison::Untracked => ("OLD · (not tracked)", "NEW · working tree"),
        DiffComparison::Commit => ("OLD · parent", "NEW · commit"),
    }
}
