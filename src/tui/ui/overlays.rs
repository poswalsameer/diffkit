//! Help and error overlays, centered and clipped to the terminal.

use super::theme;
use crate::{
    display::escape_text,
    tui::app::{App, Overlay},
};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};

/// Every key binding, grouped. Also the source for the help overlay.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Files",
        &[
            ("j / ↓", "next file"),
            ("k / ↑", "previous file"),
            ("Home / End", "first / last file"),
            ("Ctrl+D / Ctrl+U", "page down / up in the file list"),
            ("Enter", "load (or reload) the selected file"),
        ],
    ),
    (
        "Comparison",
        &[
            ("1", "staged (HEAD → index)"),
            ("2", "unstaged (index → working tree)"),
            ("3", "combined (HEAD → working tree)"),
            ("4", "untracked files"),
            ("Tab / Shift+Tab", "next / previous comparison"),
        ],
    ),
    (
        "Diff",
        &[
            ("J / K", "scroll one line down / up"),
            ("PgDn / Space / PgUp", "scroll one page"),
            ("g / G", "top / bottom"),
            ("] / [", "next / previous hunk"),
            ("l / → , h / ←", "scroll right / left"),
            ("0", "back to the first column"),
        ],
    ),
    (
        "Layout",
        &[
            ("b", "show / hide the file list"),
            ("< / >", "narrower / wider file list"),
        ],
    ),
    (
        "General",
        &[
            ("r", "refresh from Git (re-reads everything)"),
            ("?", "toggle this help"),
            ("Esc", "close overlay / clear message"),
            ("q / Ctrl+C", "quit"),
        ],
    ),
];

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2)).max(1);
    let h = height.min(area.height.saturating_sub(2)).max(1);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    match app.overlay() {
        None => {}
        Some(Overlay::Help) => help(frame, area),
        Some(Overlay::Error { title, message }) => {
            let rect = centered(area, 70, 9);
            frame.render_widget(Clear, rect);
            let block = Block::bordered().title(Span::styled(
                format!(" {} ", escape_text(title)),
                theme::error(),
            ));
            let p = Paragraph::new(vec![
                Line::raw(escape_text(message)),
                Line::raw(""),
                Line::styled("Esc close · r retry · q quit", theme::muted()),
            ])
            .wrap(Wrap { trim: true })
            .block(block);
            frame.render_widget(p, rect);
        }
    }
}

fn help(frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    for (group, keys) in KEYS {
        lines.push(Line::styled(*group, theme::accent().patch(theme::title())));
        for (key, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<22}"), theme::title()),
                Span::raw(*what),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Read-only: Diffkit never stages, commits, fetches, or edits files.",
        theme::muted(),
    ));
    lines.push(Line::styled(
        "Upstream counts use local refs and may be stale.",
        theme::muted(),
    ));
    let rect = centered(area, 72, lines.len() as u16 + 2);
    frame.render_widget(Clear, rect);
    let block = Block::bordered().title(Span::styled(" Help — ? or Esc to close ", theme::title()));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}
