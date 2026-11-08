//! Screen geometry, as pure functions of the terminal size. Shared by state
//! updates (scroll bounds) and rendering, so both always agree.

use ratatui::layout::Rect;

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
pub const DEFAULT_SIDEBAR_WIDTH: u16 = 36;
pub const MIN_SIDEBAR_WIDTH: u16 = 16;
pub const SIDEBAR_STEP: u16 = 4;
/// Narrowest content column for side-by-side text to be readable.
pub const MIN_TEXT_WIDTH: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenLayout {
    pub header: Rect,
    pub tabs: Rect,
    /// `None` when hidden by the user.
    pub sidebar: Option<Rect>,
    pub diff: Rect,
    pub status: Rect,
    pub footer: Rect,
}

/// Lays out the screen, or returns `None` if the terminal is too small.
pub fn screen(area: Rect, sidebar_width: u16, sidebar_visible: bool) -> Option<ScreenLayout> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);
    let main = Rect::new(area.x, area.y + 2, area.width, area.height - 4);
    let (sidebar, diff) = if sidebar_visible {
        let w = effective_sidebar_width(area.width, sidebar_width);
        (
            Some(Rect::new(main.x, main.y, w, main.height)),
            Rect::new(main.x + w, main.y, main.width - w, main.height),
        )
    } else {
        (None, main)
    };
    Some(ScreenLayout {
        header: row(area.y),
        tabs: row(area.y + 1),
        sidebar,
        diff,
        status: row(area.y + area.height - 2),
        footer: row(area.y + area.height - 1),
    })
}

/// The sidebar never takes more than 40% of the width.
pub fn effective_sidebar_width(total: u16, requested: u16) -> u16 {
    requested.clamp(MIN_SIDEBAR_WIDTH, (total * 2 / 5).max(MIN_SIDEBAR_WIDTH))
}

/// Rows of file entries visible in the sidebar.
pub fn sidebar_rows(sidebar: Rect) -> usize {
    sidebar.height.saturating_sub(2) as usize
}

/// Regions inside the bordered diff panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffAreas {
    pub inner: Rect,
    /// File metadata line.
    pub meta: Rect,
    /// Column titles ("OLD (HEAD)", "NEW (index)").
    pub labels: Rect,
    /// All diff rows, both sides.
    pub rows: Rect,
    pub old_gutter: Rect,
    pub old_text: Rect,
    pub separator: Rect,
    pub new_gutter: Rect,
    pub new_text: Rect,
}

/// Gutter: right-aligned line number, a space, a change marker, a space.
pub fn gutter_width(number_width: usize) -> u16 {
    number_width as u16 + 3
}

pub fn diff_inner(panel: Rect) -> Rect {
    Rect::new(
        panel.x + 1,
        panel.y + 1,
        panel.width.saturating_sub(2),
        panel.height.saturating_sub(2),
    )
}

/// Splits the diff panel for side-by-side rows. Returns the width needed
/// when the panel is too narrow for two readable columns.
pub fn diff_areas(panel: Rect, number_width: usize) -> Result<DiffAreas, u16> {
    let inner = diff_inner(panel);
    let gutter = gutter_width(number_width);
    let needed = 2 * gutter + 1 + 2 * MIN_TEXT_WIDTH + 2;
    if inner.width + 2 < needed || inner.height < 3 {
        return Err(needed);
    }
    let text = (inner.width - 2 * gutter - 1) / 2;
    let rows = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
    let col = |x: u16, w: u16| Rect::new(x, rows.y, w, rows.height);
    let old_gutter = col(inner.x, gutter);
    let old_text = col(old_gutter.right(), text);
    let separator = col(old_text.right(), 1);
    let new_gutter = col(separator.right(), gutter);
    let new_text = col(new_gutter.right(), inner.right() - new_gutter.right());
    Ok(DiffAreas {
        inner,
        meta: Rect::new(inner.x, inner.y, inner.width, 1),
        labels: Rect::new(inner.x, inner.y + 1, inner.width, 1),
        rows,
        old_gutter,
        old_text,
        separator,
        new_gutter,
        new_text,
    })
}
