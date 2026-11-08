//! Header (repository, branch, upstream) and comparison tab bar.

use super::theme;
use crate::{
    helpers::display::{display_path, escape_text},
    tui::app::App,
    types::{
        diff::DiffComparison,
        repository::{CommitTrackingInfo, HeadState, RepositoryKind, UpstreamInfo},
    },
};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let s = app.snapshot();
    let repo = s
        .repository
        .root
        .file_name()
        .map(|n| display_path(std::path::Path::new(n)))
        .unwrap_or_else(|| display_path(&s.repository.root));
    let mut spans = vec![
        Span::styled(" DIFFKIT ", theme::brand()),
        Span::raw(" "),
        Span::styled(repo, theme::title()),
    ];
    if s.repository.kind == RepositoryKind::LinkedWorktree {
        spans.push(Span::styled(" [worktree]", theme::muted()));
    }
    if s.repository.shallow {
        spans.push(Span::styled(" [shallow]", theme::muted()));
    }
    spans.push(Span::styled(" │ ", theme::muted()));
    match &s.branch.head {
        HeadState::Branch(name) => spans.push(Span::styled(
            format!("⎇ {}", escape_text(name)),
            theme::accent(),
        )),
        HeadState::Unborn(name) => {
            spans.push(Span::styled(
                format!("⎇ {}", escape_text(name)),
                theme::accent(),
            ));
            spans.push(Span::styled(" (no commits yet)", theme::muted()));
        }
        HeadState::Detached => {
            let at = s
                .branch
                .head_commit
                .as_ref()
                .map(|c| c.as_str()[..12.min(c.as_str().len())].to_owned())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("HEAD detached at {at}"),
                theme::warning(),
            ));
        }
        HeadState::Unknown => spans.push(Span::styled("HEAD unknown", theme::warning())),
    }
    for op in &s.operations {
        spans.push(Span::styled(" │ ", theme::muted()));
        spans.push(Span::styled(escape_text(&op.label()), theme::warning()));
    }
    let conflicts = s.status.conflicted_count();
    if conflicts > 0 {
        spans.push(Span::styled(
            format!(
                " ⚠ {conflicts} conflict{}",
                if conflicts == 1 { "" } else { "s" }
            ),
            theme::conflict(),
        ));
    }
    // An upstream is usually a remote-tracking ref: Git's record of the
    // remote branch as of the last fetch, never the server's current state.
    let source = |full_ref: &Option<String>| match full_ref {
        Some(r) if r.starts_with("refs/heads/") => "local branch",
        _ => "remote-tracking ref, as last fetched",
    };
    match (&s.upstream, s.commit_tracking) {
        (
            UpstreamInfo::Available { name, full_ref, .. },
            CommitTrackingInfo::Compared { ahead, behind },
        ) => {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(format!("↑{ahead}"), theme::added()));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(format!("↓{behind}"), theme::removed()));
            spans.push(Span::styled(
                format!(" vs {} ({})", escape_text(name), source(full_ref)),
                theme::muted(),
            ));
        }
        (UpstreamInfo::Available { name, full_ref, .. }, _) => {
            spans.push(Span::styled(
                format!(" tracking {} ({})", escape_text(name), source(full_ref)),
                theme::muted(),
            ));
        }
        (UpstreamInfo::Missing { name }, _) => {
            spans.push(Span::styled(
                format!(" upstream {} missing locally", escape_text(name)),
                theme::warning(),
            ));
        }
        (UpstreamInfo::NotConfigured, _) => {
            spans.push(Span::styled(" (no upstream)", theme::muted()))
        }
        (UpstreamInfo::NotApplicable, _) => {}
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (i, c) in DiffComparison::ALL.into_iter().enumerate() {
        let state = app.mode_state(c);
        let count = match &state.list {
            Ok(l) => l.len().to_string(),
            Err(_) => "!".into(),
        };
        let label = format!(" {} {} {} ", i + 1, theme::mode_title(c), count);
        let style = if c == app.mode() {
            theme::selected().patch(theme::title())
        } else if state.list.is_err() {
            theme::error()
        } else {
            theme::muted()
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(app.mode().description(), theme::muted()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
