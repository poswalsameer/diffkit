//! Status line (messages, position) and key hints.

use super::theme;
use crate::{
    display::escape_text,
    tui::app::{App, StatusKind, SyncPhase},
};
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Position text such as `file 2/5 · hunk 1/3 · rows 1-30 of 120 · col 0`.
pub fn position(app: &App, visible_rows: Option<usize>) -> String {
    let (n, selected) = match app.history() {
        Some(h) => match &h.open {
            Some(c) => (c.files().len(), c.selected),
            None => {
                let n = h.list().len();
                return if n == 0 {
                    "no commits".into()
                } else {
                    format!("commit {}/{n}", h.selected + 1)
                };
            }
        },
        None => {
            let state = app.mode_state(app.mode());
            (state.files().len(), state.selected)
        }
    };
    if n == 0 {
        return "no files".into();
    }
    let mut parts = vec![format!("file {}/{n}", selected + 1)];
    if let Some(v) = app.view() {
        let hunks = v.hunk_starts().len();
        if hunks > 0 {
            let current = app.scroll().hunk.map_or(0, |h| h + 1);
            parts.push(format!("hunk {current}/{hunks}"));
            let total = v.row_count();
            let first = app.scroll().top + 1;
            let last = visible_rows.map_or(total, |h| (app.scroll().top + h).min(total));
            parts.push(format!("rows {first}-{last} of {total}"));
            if app.scroll().left > 0 {
                parts.push(format!("col {}", app.scroll().left));
            }
        }
    }
    parts.join(" · ")
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let visible = app.screen().and_then(|s| {
        let w = app.view().map_or(3, |v| v.number_width());
        crate::tui::layout::diff_areas(s.diff, w)
            .ok()
            .map(|a| a.rows.height as usize)
    });
    let mut spans = Vec::new();
    if let Some(status) = app.status() {
        let style = match status.kind {
            StatusKind::Info => theme::accent(),
            StatusKind::Warning => theme::warning(),
            StatusKind::Error => theme::error(),
        };
        spans.push(Span::styled(escape_text(&status.text), style));
        spans.push(Span::styled("  │  ", theme::muted()));
    }
    spans.push(Span::styled(position(app, visible), theme::muted()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    let (text, style) = sync_indicator(app);
    if !text.is_empty() {
        let w = (crate::display::width(&text) as u16 + 1).min(area.width);
        let rect = Rect::new(area.right() - w, area.y, w, 1);
        frame.render_widget(
            Paragraph::new(Line::styled(text, style)).alignment(Alignment::Right),
            rect,
        );
    }
}

/// Text and style of the synchronization indicator.
pub fn sync_indicator(app: &App) -> (String, Style) {
    let s = app.sync_state();
    if s.phase == SyncPhase::Synchronizing {
        return (" ↻ Synchronizing".into(), theme::accent());
    }
    if s.watching == Some(false) {
        let off = s
            .watcher_message
            .as_deref()
            .is_some_and(|m| m.contains("--no-watch"));
        return if off {
            (" ○ Auto-sync off".into(), theme::muted())
        } else {
            (" ! Watcher unavailable".into(), theme::warning())
        };
    }
    match &s.phase {
        SyncPhase::Manual => (String::new(), theme::muted()),
        SyncPhase::Synced => (" ● Synced".into(), theme::added()),
        SyncPhase::ChangesDetected => (" ◌ Changes detected".into(), theme::modified()),
        SyncPhase::Synchronizing => (" ↻ Synchronizing".into(), theme::accent()),
        SyncPhase::Failed(_) => (" ! Refresh required".into(), theme::error()),
    }
}

pub fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints: &[(&str, &str)] = match app.history() {
        Some(h) if h.open.is_some() => &[
            ("j/k", "files"),
            ("J/K PgUp/PgDn", "scroll"),
            ("[ ]", "hunks"),
            ("Esc", "back to commits"),
            ("L", "close history"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Some(_) => &[
            ("j/k", "commits"),
            ("Enter", "open commit"),
            ("r", "reload"),
            ("Esc/L", "back to working tree"),
            ("?", "help"),
            ("q", "quit"),
        ],
        None => &[
            ("j/k", "files"),
            ("J/K PgUp/PgDn", "scroll"),
            ("[ ]", "hunks"),
            ("h/l", "left/right"),
            ("1-4 Tab", "comparison"),
            ("L", "history"),
            ("r", "refresh"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    let mut spans = Vec::new();
    for &(key, what) in hints {
        spans.push(Span::styled(key, theme::title()));
        spans.push(Span::styled(format!(" {what}  "), theme::muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
