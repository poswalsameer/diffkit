//! Rendering. Every function here reads [`App`] and writes to the frame;
//! none of them runs Git, reads files, parses, or aligns.

mod diff_panel;
mod header;
mod history;
mod overlays;
mod sidebar;
mod status;
pub mod theme;

use super::{app::App, layout};
use ratatui::{
    layout::{Alignment, Rect},
    text::Line,
    widgets::{Paragraph, Wrap},
    Frame,
};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let Some(screen) = layout::screen(area, app.sidebar().width, app.sidebar().visible) else {
        too_small(frame, area);
        return;
    };
    header::render(frame, app, screen.header);
    match app.history() {
        Some(h) => {
            history::render_bar(frame, app, screen.tabs);
            let commit_open = h.open.is_some();
            if let Some(sidebar) = screen.sidebar {
                if commit_open {
                    sidebar::render(frame, app, sidebar);
                } else {
                    history::render_list(frame, app, sidebar);
                }
            }
            if commit_open {
                diff_panel::render(frame, app, screen.diff);
            } else {
                history::render_details(frame, app, screen.diff);
            }
        }
        None => {
            header::render_tabs(frame, app, screen.tabs);
            if let Some(sidebar) = screen.sidebar {
                sidebar::render(frame, app, sidebar);
            }
            diff_panel::render(frame, app, screen.diff);
        }
    }
    status::render(frame, app, screen.status);
    status::render_footer(frame, app, screen.footer);
    overlays::render(frame, app, area);
}

fn too_small(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::styled("Terminal too small", theme::warning()),
        Line::raw(format!(
            "{}x{} — Diffkit needs at least {}x{}.",
            area.width,
            area.height,
            layout::MIN_WIDTH,
            layout::MIN_HEIGHT
        )),
        Line::raw("Enlarge the window, or press q to quit."),
    ];
    let y = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    let rect = Rect::new(area.x, y, area.width, area.height.min(lines.len() as u16));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        rect,
    );
}
