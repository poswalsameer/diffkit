//! Status line (messages, position) and key hints.

use super::theme;
use crate::{
    display::escape_text,
    tui::app::{App, StatusKind},
};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Position text such as `file 2/5 · hunk 1/3 · rows 1-30 of 120 · col 0`.
pub fn position(app: &App, visible_rows: Option<usize>) -> String {
    let state = app.mode_state(app.mode());
    let n = state.files().len();
    if n == 0 {
        return "no files".into();
    }
    let mut parts = vec![format!("file {}/{n}", state.selected + 1)];
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
}

pub fn render_footer(frame: &mut Frame, _app: &App, area: Rect) {
    let hints = [
        ("j/k", "files"),
        ("J/K PgUp/PgDn", "scroll"),
        ("[ ]", "hunks"),
        ("h/l", "left/right"),
        ("1-4 Tab", "comparison"),
        ("r", "refresh"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = Vec::new();
    for (key, what) in hints {
        spans.push(Span::styled(key, theme::title()));
        spans.push(Span::styled(format!(" {what}  "), theme::muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
