//! Changed-file list for the active comparison.

use super::theme;
use crate::{
    diff::{BinaryStatus, DiffFile},
    display::{display_path, truncate_start, width},
    tui::{app::App, layout},
};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

/// Display label for a file: `old → new` for renames and copies.
pub fn file_label(f: &DiffFile) -> String {
    match (&f.old_path, &f.new_path) {
        (Some(o), Some(n)) if o != n => format!("{} → {}", display_path(o), display_path(n)),
        _ => display_path(f.path()),
    }
}

fn stats_label(f: &DiffFile) -> Option<String> {
    if f.binary == BinaryStatus::Binary {
        return Some("bin".into());
    }
    f.stats.map(|s| format!("+{} -{}", s.added, s.removed))
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let state = app.mode_state(app.mode());
    let title = format!(" CHANGES ({}) ", state.files().len());
    let block = Block::bordered().title(Span::styled(title, theme::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Err(message) = &state.list {
        let p = Paragraph::new(vec![
            Line::styled("Could not list changes:", theme::error()),
            Line::raw(message.clone()),
            Line::styled("Press r to refresh.", theme::muted()),
        ])
        .wrap(Wrap { trim: true });
        frame.render_widget(p, inner);
        return;
    }
    let files = state.files();
    if files.is_empty() {
        let p = Paragraph::new(vec![
            Line::styled(
                format!(
                    "No {} changes",
                    theme::mode_title(app.mode()).to_lowercase()
                ),
                theme::muted(),
            ),
            Line::styled("Tab or 1-4 switches comparison.", theme::muted()),
        ])
        .wrap(Wrap { trim: true });
        frame.render_widget(p, inner);
        return;
    }
    let rows = layout::sidebar_rows(area);
    let w = inner.width as usize;
    let lines: Vec<Line> = files
        .iter()
        .enumerate()
        .skip(state.offset)
        .take(rows)
        .map(|(i, f)| {
            let (code, code_style) = theme::status_code(f);
            let flag = if f.conflicted && f.kind != crate::diff::FileChangeKind::Unmerged {
                "!"
            } else {
                " "
            };
            let prefix = format!("{code:<2}{flag}");
            let stats = stats_label(f).filter(|s| w > 24 && width(s) + 8 < w);
            let room =
                w.saturating_sub(width(&prefix) + stats.as_ref().map_or(0, |s| width(s) + 1));
            let label = truncate_start(&file_label(f), room);
            let pad = room.saturating_sub(width(&label));
            let mut spans = vec![
                Span::styled(prefix, code_style),
                Span::raw(label),
                Span::raw(" ".repeat(pad)),
            ];
            if let Some(s) = stats {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(s, theme::muted()));
            }
            let line = Line::from(spans);
            if i == state.selected {
                line.patch_style(theme::selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
