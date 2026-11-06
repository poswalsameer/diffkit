//! Changed-file list for the active comparison, or for the commit opened
//! on the history screen.

use super::theme;
use crate::{
    diff::{BinaryStatus, DiffFile, FileChangeKind},
    display::{display_path, truncate_start, width},
    sync::reconcile::ListResult,
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

/// A file list and its position, from either screen.
struct Files<'a> {
    title: String,
    list: &'a ListResult,
    selected: usize,
    offset: usize,
    /// Shown when the list is empty.
    empty: String,
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let files = match app.history().and_then(|h| h.open.as_ref()) {
        Some(c) => Files {
            title: format!(
                " FILES IN {} ({}) ",
                crate::display::escape_text(&c.commit.short_id),
                c.files().len()
            ),
            list: &c.list,
            selected: c.selected,
            offset: c.offset,
            empty: "This commit changes no files.".into(),
        },
        None => {
            let state = app.mode_state(app.mode());
            Files {
                title: format!(" CHANGES ({}) ", state.files().len()),
                list: &state.list,
                selected: state.selected,
                offset: state.offset,
                empty: format!(
                    "No {} changes",
                    theme::mode_title(app.mode()).to_lowercase()
                ),
            }
        }
    };
    render_files(frame, app, area, files);
}

fn render_files(frame: &mut Frame, app: &App, area: Rect, view: Files) {
    let Files {
        title,
        list,
        selected,
        offset,
        empty,
    } = view;
    let block = Block::bordered().title(Span::styled(title, theme::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let files = match list {
        Err(message) => {
            let mut lines = vec![
                Line::styled("Could not list changes:", theme::error()),
                Line::raw(crate::display::escape_text(message)),
            ];
            // A commit's files are fixed; only working-tree lists can
            // recover on refresh.
            if app.history().is_none() {
                lines.push(Line::styled("Press r to refresh.", theme::muted()));
            }
            let p = Paragraph::new(lines).wrap(Wrap { trim: true });
            frame.render_widget(p, inner);
            return;
        }
        Ok(l) => &l.files,
    };
    if files.is_empty() {
        let mut lines = vec![Line::styled(empty, theme::muted())];
        if app.history().is_none() {
            lines.push(Line::styled(
                "Tab or 1-4 switches comparison.",
                theme::muted(),
            ));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        return;
    }
    let rows = layout::sidebar_rows(area);
    let w = inner.width as usize;
    let lines: Vec<Line> = files
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .map(|(i, f)| {
            let (code, code_style) = theme::status_code(f);
            let flag = if f.conflicted && f.kind != FileChangeKind::Unmerged {
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
            if i == selected {
                line.patch_style(theme::selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
