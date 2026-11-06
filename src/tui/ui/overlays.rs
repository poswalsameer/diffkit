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
        "History (local commits only)",
        &[
            ("L", "open / close recent history"),
            ("Enter", "list a commit's files / reload a file"),
            ("Esc", "commit → history → working tree"),
            ("r", "reload the history list"),
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
            ("r", "refresh now (same pipeline as automatic sync)"),
            ("D", "synchronization and cache diagnostics"),
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
        Some(Overlay::Diagnostics) => diagnostics(frame, app, area),
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
        "↑/↓ compare with the upstream as last fetched; Diffkit never fetches.",
        theme::muted(),
    ));
    let rect = centered(area, 72, lines.len() as u16 + 2);
    frame.render_widget(Clear, rect);
    let block = Block::bordered().title(Span::styled(" Help — ? or Esc to close ", theme::title()));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

fn diagnostics(frame: &mut Frame, app: &App, area: Rect) {
    let d = app.sync_state().diagnostics;
    let c = app.cache_stats();
    let ms = |t: Option<std::time::Duration>| {
        t.map_or("—".to_string(), |t| format!("{} ms", t.as_millis()))
    };
    let mean = if d.reconciles_completed + d.reconcile_failures > 0 {
        Some(d.total_reconcile / (d.reconciles_completed + d.reconcile_failures) as u32)
    } else {
        None
    };
    let kib = |b: usize| format!("{:.1} MiB", b as f64 / (1 << 20) as f64);
    let rows: Vec<(String, String)> = vec![
        ("Synchronization".into(), String::new()),
        (
            "  watcher".into(),
            match (
                &app.sync_state().watching,
                &app.sync_state().watcher_message,
            ) {
                (Some(true), _) => "watching".into(),
                (_, Some(m)) => escape_text(m),
                _ => "not started (manual refresh only)".into(),
            },
        ),
        ("  filesystem events".into(), d.fs_events.to_string()),
        (
            "  relevant / coalesced".into(),
            format!("{} / {}", d.relevant_events, d.coalesced_events),
        ),
        (
            "  queue overflows (dropped)".into(),
            format!("{} ({})", d.overflows, d.dropped_events),
        ),
        (
            "  reconciliations requested".into(),
            d.reconcile_requests.to_string(),
        ),
        (
            "  completed / unchanged / failed".into(),
            format!(
                "{} / {} / {}",
                d.reconciles_completed, d.reconciles_unchanged, d.reconcile_failures
            ),
        ),
        (
            "  last / mean duration".into(),
            format!("{} / {}", ms(d.last_reconcile), ms(mean)),
        ),
        ("  lists reused".into(), d.lists_reused.to_string()),
        (
            "  restarted / withheld (Git moving)".into(),
            format!("{} / {}", d.reconcile_retries, d.reconciles_withheld),
        ),
        (
            "  Git processes (sync worker)".into(),
            d.git_calls.to_string(),
        ),
        (
            "  watcher errors / restarts".into(),
            format!("{} / {}", d.watcher_errors, d.watcher_restarts),
        ),
        ("Diff cache".into(), String::new()),
        ("  entries".into(), c.entries.to_string()),
        (
            "  approx. memory (peak) / budget".into(),
            format!(
                "{} ({}) / {}",
                kib(c.bytes),
                kib(c.peak_bytes),
                kib(c.budget_bytes)
            ),
        ),
        (
            "  hits / misses".into(),
            format!("{} / {}", c.hits, c.misses),
        ),
        (
            "  evictions / invalidations".into(),
            format!("{} / {}", c.evictions, c.invalidations),
        ),
    ];
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, v)| {
            if v.is_empty() {
                Line::styled(k, theme::accent().patch(theme::title()))
            } else {
                Line::from(vec![
                    Span::styled(format!("{k:<34}"), theme::muted()),
                    Span::raw(v),
                ])
            }
        })
        .collect();
    let rect = centered(area, 70, lines.len() as u16 + 2);
    frame.render_widget(Clear, rect);
    let block = Block::bordered().title(Span::styled(
        " Diagnostics — D or Esc to close ",
        theme::title(),
    ));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}
