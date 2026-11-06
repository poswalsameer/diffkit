//! Side-by-side diff panel: old content on the left, new on the right, each
//! with its own line numbers. Rows come straight from the Phase 2 alignment
//! via [`DiffView`]; nothing is re-aligned here.

use super::{sidebar::file_label, theme};
use crate::{
    diff::{DiffRowKind, FileChangeKind},
    display::{display_path, escape_text, truncate_start},
    git::models::FileKind,
    tui::{
        app::{App, FailureKind, Loaded},
        diff_view::{Body, Cell, DiffView, Notice, ViewRow, CRLF_MARK, NO_NEWLINE_MARK},
        layout,
    },
};
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let file = app.selected_file();
    let title = match file {
        Some(f) => format!(
            " {} ",
            truncate_start(&file_label(f), area.width.saturating_sub(4) as usize)
        ),
        None => " DIFF ".into(),
    };
    let block = Block::bordered().title(Span::styled(title, theme::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match (file, app.loaded()) {
        (None, _) => {
            let state = app.mode_state(app.mode());
            let text = if state.list.is_err() {
                "The file list could not be loaded."
            } else {
                "Nothing to show in this comparison."
            };
            message(frame, inner, vec![Line::styled(text, theme::muted())]);
        }
        (Some(_), None) => message(frame, inner, vec![Line::styled("Loading…", theme::muted())]),
        (Some(_), Some(Loaded::Failed(f))) => {
            let (heading, style) = match f.kind {
                FailureKind::Stale => (
                    "This file changed since the list was loaded",
                    theme::warning(),
                ),
                FailureKind::Vanished => ("This file no longer exists", theme::warning()),
                FailureKind::Other => ("Could not load this diff", theme::error()),
            };
            message(
                frame,
                inner,
                vec![
                    Line::styled(heading, style),
                    Line::raw(""),
                    Line::raw(escape_text(&f.message)),
                    Line::raw(""),
                    Line::styled("r refresh · Enter retry", theme::muted()),
                ],
            );
        }
        (Some(_), Some(Loaded::View(v))) => view(frame, app, v, area, inner),
    }
}

fn message(frame: &mut Frame, inner: Rect, lines: Vec<Line>) {
    let h = (lines.len() as u16).min(inner.height);
    let y = inner.y + inner.height.saturating_sub(h) / 2;
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        Rect::new(inner.x, y, inner.width, inner.height - (y - inner.y)),
    );
}

fn meta_line(v: &DiffView) -> Line<'static> {
    let f = &v.file;
    let mut spans = Vec::new();
    if f.conflicted || f.kind == FileChangeKind::Unmerged {
        spans.push(Span::styled(
            " ⚠ CONFLICT ",
            theme::conflict().patch(theme::selected()),
        ));
        spans.push(Span::raw(" "));
    }
    let (_, kind_style) = theme::status_code(f);
    spans.push(Span::styled(theme::kind_name(f), kind_style));
    if let (Some(old), Some(new)) = (&f.old_path, &f.new_path) {
        if old != new {
            let verb = if f.kind == FileChangeKind::Copied {
                "from"
            } else {
                "was"
            };
            spans.push(Span::styled(
                format!(" · {verb} {}", display_path(old)),
                theme::muted(),
            ));
        }
    }
    if let Some(s) = f.similarity {
        spans.push(Span::styled(format!(" ({s}% similar)"), theme::muted()));
    }
    match (f.old_mode, f.new_mode) {
        (Some(o), Some(n)) if o != n => {
            spans.push(Span::styled(
                format!(" · mode {:o} → {:o}", o.0, n.0),
                theme::modified(),
            ));
        }
        (o, n) => {
            if let Some(m) = n.or(o) {
                let kind = match m.kind() {
                    Some(FileKind::Symlink) => {
                        " · symlink (target shown, not followed)".to_string()
                    }
                    Some(FileKind::Submodule) => " · submodule".to_string(),
                    Some(FileKind::Executable) => " · executable".to_string(),
                    _ => format!(" · {:o}", m.0),
                };
                spans.push(Span::styled(kind, theme::muted()));
            }
        }
    }
    if let Body::Rows {
        hunk_starts,
        bounded_alignment,
        ..
    } = &v.body
    {
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(format!("+{}", v.added), theme::added()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("-{}", v.removed), theme::removed()));
        let n = hunk_starts.len();
        spans.push(Span::styled(
            format!(" · {n} hunk{}", if n == 1 { "" } else { "s" }),
            theme::muted(),
        ));
        if *bounded_alignment {
            spans.push(Span::styled(" · alignment bounded", theme::warning()));
        }
    }
    Line::from(spans)
}

fn notice_lines(v: &DiffView, notice: &Notice) -> Vec<Line<'static>> {
    let size = |s: Option<u64>| s.map_or("—".to_string(), |b| format!("{b} bytes"));
    match notice {
        Notice::Binary(b) => vec![
            Line::styled("Binary file — content preview unavailable.", theme::title()),
            Line::raw(""),
            Line::raw(format!("Size: {} → {}", size(b.old_size), size(b.new_size))),
        ],
        Notice::Submodule(s) => {
            let id = |c: &Option<crate::git::models::ObjectId>, none: &str| {
                c.as_ref()
                    .map_or(none.to_string(), |c| c.as_str()[..12].to_string())
            };
            let mut lines = vec![
                Line::styled("Submodule change", theme::title()),
                Line::raw(""),
                Line::raw(format!(
                    "Commit {} → {}",
                    id(&s.old_commit, "(none)"),
                    id(&s.new_commit, "(working tree)")
                )),
            ];
            if s.new_dirty {
                lines.push(Line::styled(
                    "The submodule has uncommitted changes.",
                    theme::warning(),
                ));
            }
            lines.push(Line::styled(
                "Submodule contents are not shown.",
                theme::muted(),
            ));
            lines
        }
        Notice::Unmerged => vec![
            Line::styled("⚠ CONFLICT — this path is unmerged", theme::conflict()),
            Line::raw(""),
            Line::raw("Git has no two-way diff for an unmerged path in this comparison."),
            Line::raw(
                "The COMBINED view (3) shows HEAD → working tree, including conflict markers.",
            ),
            Line::styled(
                "Diffkit never modifies files; resolve the conflict in your editor.",
                theme::muted(),
            ),
        ],
        Notice::TooLarge { limit, size: s } => vec![
            Line::styled("File too large to display", theme::warning()),
            Line::raw(""),
            Line::raw(format!(
                "The limit is {limit} bytes; nothing partial is shown."
            )),
            Line::raw(format!("Size: {}", size(*s))),
        ],
        Notice::Unreadable(reason) => vec![
            Line::styled("This file cannot be read", theme::error()),
            Line::raw(escape_text(reason)),
        ],
        Notice::NestedRepository => vec![
            Line::styled("Nested Git repository", theme::title()),
            Line::styled("Its contents are not shown as a file.", theme::muted()),
        ],
        Notice::NoTextChanges => {
            let f = &v.file;
            let text = match f.kind {
                FileChangeKind::Renamed => "Renamed without content changes.".to_string(),
                FileChangeKind::Copied => "Copied without content changes.".to_string(),
                FileChangeKind::ModeChanged => match (f.old_mode, f.new_mode) {
                    (Some(o), Some(n)) => {
                        format!("Only the file mode changed: {:o} → {:o}.", o.0, n.0)
                    }
                    _ => "Only the file mode changed.".to_string(),
                },
                FileChangeKind::Added | FileChangeKind::Deleted => {
                    "Empty file (no lines).".to_string()
                }
                _ if f.comparison == crate::diff::DiffComparison::Untracked => {
                    "Empty file (no lines).".to_string()
                }
                _ => "No line changes.".to_string(),
            };
            vec![Line::styled(text, theme::muted())]
        }
    }
}

fn view(frame: &mut Frame, app: &App, v: &DiffView, area: Rect, inner: Rect) {
    let meta = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
    frame.render_widget(Paragraph::new(meta_line(v)), meta);
    let below = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let (rows, number_width) = match &v.body {
        Body::Rows {
            rows, number_width, ..
        } => (rows, number_width),
        Body::Notice(n) => {
            message(frame, below, notice_lines(v, n));
            return;
        }
    };
    let a = match layout::diff_areas(area, *number_width) {
        Ok(a) => a,
        Err(needed) => {
            message(
                frame,
                below,
                vec![
                    Line::styled("Too narrow for side-by-side view", theme::warning()),
                    Line::raw(format!(
                        "The diff panel needs {needed} columns; it has {}.",
                        area.width
                    )),
                    Line::styled(
                        "b hides the sidebar · < shrinks it · or widen the terminal",
                        theme::muted(),
                    ),
                ],
            );
            return;
        }
    };
    let (old_label, new_label) = theme::side_labels(app.mode());
    let labels = Line::from(vec![
        Span::raw(" ".repeat(a.old_gutter.width as usize)),
        Span::styled(old_label, theme::title()),
    ]);
    frame.render_widget(
        Paragraph::new(labels),
        Rect::new(a.labels.x, a.labels.y, a.old_text.right() - a.labels.x, 1),
    );
    let new_labels = Line::from(vec![
        Span::raw(" ".repeat(a.new_gutter.width as usize + 1)),
        Span::styled(new_label, theme::title()),
    ]);
    frame.render_widget(
        Paragraph::new(new_labels),
        Rect::new(
            a.separator.x,
            a.labels.y,
            a.inner.right() - a.separator.x,
            1,
        ),
    );

    let scroll = app.scroll();
    let height = a.rows.height as usize;
    let top = scroll.top.min(rows.len());
    let visible = &rows[top..(top + height).min(rows.len())];
    let nw = *number_width;
    let mut old_gutter = Vec::with_capacity(height);
    let mut old_text = Vec::with_capacity(height);
    let mut separator = Vec::with_capacity(height);
    let mut new_gutter = Vec::with_capacity(height);
    let mut new_text = Vec::with_capacity(height);
    let mut hunks = Vec::new();
    for (y, row) in visible.iter().enumerate() {
        match row {
            ViewRow::Hunk { index, header } => {
                for col in [
                    &mut old_gutter,
                    &mut old_text,
                    &mut separator,
                    &mut new_gutter,
                    &mut new_text,
                ] {
                    col.push(Line::raw(""));
                }
                hunks.push((y as u16, *index, header.as_str()));
            }
            ViewRow::Line { kind, old, new } => {
                old_gutter.push(gutter(old.as_ref(), *kind, true, nw));
                old_text.push(cell_line(old.as_ref(), theme::side(*kind, true)));
                separator.push(Line::styled("│", theme::muted()));
                new_gutter.push(gutter(new.as_ref(), *kind, false, nw));
                new_text.push(cell_line(new.as_ref(), theme::side(*kind, false)));
            }
        }
    }
    let left = scroll.left.min(u16::MAX as usize) as u16;
    frame.render_widget(Paragraph::new(old_gutter), a.old_gutter);
    frame.render_widget(Paragraph::new(old_text).scroll((0, left)), a.old_text);
    frame.render_widget(Paragraph::new(separator), a.separator);
    frame.render_widget(Paragraph::new(new_gutter), a.new_gutter);
    frame.render_widget(Paragraph::new(new_text).scroll((0, left)), a.new_text);
    for (y, index, header) in hunks {
        let selected = scroll.hunk == Some(index);
        let (mark, style) = if selected {
            ("▶ ", theme::accent().patch(theme::title()))
        } else {
            ("  ", theme::accent())
        };
        let line = Line::from(vec![Span::styled(mark, style), Span::styled(header, style)]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(a.rows.x, a.rows.y + y, a.rows.width, 1),
        );
    }
}

fn gutter(cell: Option<&Cell>, kind: DiffRowKind, old_side: bool, width: usize) -> Line<'static> {
    match cell {
        Some(c) => {
            let (mark, mark_style) = theme::marker(kind, old_side);
            Line::from(vec![
                Span::styled(format!("{:>width$} ", c.number), theme::muted()),
                Span::styled(mark, mark_style),
            ])
        }
        None => Line::raw(""),
    }
}

fn cell_line(cell: Option<&Cell>, style: Style) -> Line<'_> {
    let Some(c) = cell else { return Line::raw("") };
    let mut spans: Vec<Span> = c
        .segments
        .iter()
        .map(|s| {
            Span::styled(
                s.text.as_str(),
                if s.escaped { theme::escape() } else { style },
            )
        })
        .collect();
    match c.ending {
        crate::diff::LineEnding::None => spans.push(Span::styled(NO_NEWLINE_MARK, theme::muted())),
        crate::diff::LineEnding::CrLf => spans.push(Span::styled(CRLF_MARK, theme::muted())),
        crate::diff::LineEnding::Lf => {}
    }
    Line::from(spans)
}
