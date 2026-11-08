//! The history screen: recent local commits, the selected commit's
//! metadata, and the bar describing an opened commit. Every value from the
//! repository (names, e-mails, messages, refs) is escaped before display.

use super::theme;
use crate::{
    git::history::CommitInfo,
    helpers::display::{escape_text, truncate_end, width},
    tui::{app::App, layout},
};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

/// `2026-11-06` from `2026-11-06T22:41:08+05:30`.
fn day(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

/// The commit list in the sidebar area.
pub fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let Some(h) = app.history() else { return };
    let count = match &h.commits {
        Ok(list) if list.truncated => format!("{}+", list.commits.len()),
        Ok(list) => list.commits.len().to_string(),
        Err(_) => "!".into(),
    };
    let block =
        Block::bordered().title(Span::styled(format!(" HISTORY ({count}) "), theme::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let commits = match &h.commits {
        Err(e) => {
            let p = Paragraph::new(vec![
                Line::styled("Could not read the history:", theme::error()),
                Line::raw(escape_text(e)),
            ])
            .wrap(Wrap { trim: true });
            frame.render_widget(p, inner);
            return;
        }
        Ok(list) if list.commits.is_empty() => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "No commits yet on this branch.",
                    theme::muted(),
                ))
                .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        }
        Ok(list) => &list.commits,
    };
    let rows = layout::sidebar_rows(area);
    let w = inner.width as usize;
    let lines: Vec<Line> = commits
        .iter()
        .enumerate()
        .skip(h.offset)
        .take(rows)
        .map(|(i, c)| {
            let id = escape_text(&c.short_id);
            // In a narrow list the subject matters more than the date.
            let date = if w >= 36 {
                escape_text(day(&c.author.date))
            } else {
                String::new()
            };
            let lead = if date.is_empty() {
                format!("{id} ")
            } else {
                format!("{id} {date} ")
            };
            let marker = if c.is_merge() { "⑂ " } else { "" };
            let room = w.saturating_sub(width(&lead) + width(marker));
            let subject = truncate_end(&escape_text(c.subject()), room);
            let pad = room.saturating_sub(width(&subject));
            let line = Line::from(vec![
                Span::styled(id, theme::accent()),
                Span::raw(" "),
                Span::styled(
                    if date.is_empty() {
                        date
                    } else {
                        format!("{date} ")
                    },
                    theme::muted(),
                ),
                Span::styled(marker, theme::muted()),
                Span::raw(subject),
                Span::raw(" ".repeat(pad)),
            ]);
            if i == h.selected {
                line.patch_style(theme::selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn parents_line(c: &CommitInfo) -> String {
    if c.parents.is_empty() {
        return if c.shallow_boundary {
            "not available (shallow clone boundary)".into()
        } else {
            "none (root commit)".into()
        };
    }
    let ids: Vec<&str> = c.parents.iter().map(|p| &p.as_str()[..12]).collect();
    if c.is_merge() {
        format!("{} (merge; diffs use the first parent)", ids.join(" "))
    } else {
        ids.join(" ")
    }
}

/// Full metadata of a commit, one labelled line per field, then the message.
pub fn metadata_lines(c: &CommitInfo) -> Vec<Line<'static>> {
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<10}"), theme::muted()),
            Span::raw(value),
        ])
    };
    let who = |s: &crate::git::history::Signature| {
        format!(
            "{} <{}>  {}",
            escape_text(&s.name),
            escape_text(&s.email),
            escape_text(&s.date)
        )
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("commit    ", theme::muted()),
            Span::styled(escape_text(c.id.as_str()), theme::accent()),
        ]),
        field("short", escape_text(&c.short_id)),
        field("parents", parents_line(c)),
        field("author", who(&c.author)),
        field("committer", who(&c.committer)),
        Line::raw(""),
    ];
    for l in c.message.lines() {
        lines.push(Line::raw(format!("    {}", escape_text(l))));
    }
    lines
}

/// The right-hand panel while the commit list is shown.
pub fn render_details(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(Span::styled(" COMMIT ", theme::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(h) = app.history() else { return };
    let mut lines = match h.selected_commit() {
        Some(c) => metadata_lines(c),
        None => vec![Line::styled("No commit selected.", theme::muted())],
    };
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Enter lists the files this commit changed · Esc returns to the working tree",
        theme::muted(),
    ));
    if matches!(&h.commits, Ok(list) if list.truncated) {
        lines.push(Line::styled(
            format!(
                "Only the {} most recent commits are listed.",
                h.list().len()
            ),
            theme::muted(),
        ));
    }
    if app.snapshot().repository.shallow {
        lines.push(Line::styled(
            "Shallow clone: older history is not available locally.",
            theme::muted(),
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// The row under the header: what the history screen shows.
pub fn render_bar(frame: &mut Frame, app: &App, area: Rect) {
    let Some(h) = app.history() else { return };
    let spans = match &h.open {
        Some(c) => vec![
            Span::styled(" COMMIT ", theme::selected().patch(theme::title())),
            Span::raw(" "),
            Span::styled(escape_text(&c.commit.short_id), theme::accent()),
            Span::raw(" "),
            Span::raw(escape_text(c.commit.subject())),
            Span::styled(
                format!(
                    "  {} · {}",
                    escape_text(&c.commit.author.name),
                    escape_text(day(&c.commit.author.date))
                ),
                theme::muted(),
            ),
        ],
        None => vec![
            Span::styled(" HISTORY ", theme::selected().patch(theme::title())),
            Span::styled(
                "  recent commits reachable from HEAD (local history only)",
                theme::muted(),
            ),
        ],
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
