//! Screen geometry: too-small terminals, tiling, sidebar bounds, and diff
//! column widths.

use diffkit::tui::layout::*;
use ratatui::layout::Rect;

#[test]
fn too_small_terminals_have_no_layout() {
    assert!(screen(Rect::new(0, 0, 39, 30), 36, true).is_none());
    assert!(screen(Rect::new(0, 0, 120, 9), 36, true).is_none());
    assert!(screen(Rect::new(0, 0, 40, 10), 36, true).is_some());
}

#[test]
fn regions_tile_the_screen() {
    let l = screen(Rect::new(0, 0, 120, 40), 36, true).unwrap();
    assert_eq!(l.header, Rect::new(0, 0, 120, 1));
    assert_eq!(l.tabs.y, 1);
    assert_eq!(l.sidebar.unwrap(), Rect::new(0, 2, 36, 36));
    assert_eq!(l.diff, Rect::new(36, 2, 84, 36));
    assert_eq!((l.status.y, l.footer.y), (38, 39));
    let hidden = screen(Rect::new(0, 0, 120, 40), 36, false).unwrap();
    assert_eq!((hidden.sidebar, hidden.diff.width), (None, 120));
}

#[test]
fn sidebar_never_dominates() {
    assert_eq!(effective_sidebar_width(80, 200), 32);
    assert_eq!(effective_sidebar_width(200, 36), 36);
    assert_eq!(effective_sidebar_width(40, 36), 16);
    assert_eq!(effective_sidebar_width(200, 0), MIN_SIDEBAR_WIDTH);
}

#[test]
fn side_by_side_columns_fit_exactly() {
    let a = diff_areas(Rect::new(36, 2, 84, 36), 3).unwrap();
    assert_eq!(a.old_gutter.width, 6);
    assert_eq!(a.old_text.width, a.new_text.width.min(a.old_text.width));
    assert_eq!(a.new_text.right(), a.inner.right());
    assert_eq!(a.old_text.x + a.old_text.width, a.separator.x);
    assert_eq!(a.rows.height, 32);
}

#[test]
fn narrow_panel_reports_needed_width() {
    assert_eq!(diff_areas(Rect::new(0, 0, 20, 20), 3), Err(35));
}
