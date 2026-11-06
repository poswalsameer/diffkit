//! User intentions, independent of keys and rendering.

use crate::diff::DiffComparison;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SelectNextFile,
    SelectPreviousFile,
    SelectFirstFile,
    SelectLastFile,
    SidebarPageUp,
    SidebarPageDown,
    /// Load (or reload) the selected file's diff now.
    OpenSelected,
    SetComparison(DiffComparison),
    NextComparison,
    PreviousComparison,
    ScrollDown(usize),
    ScrollUp(usize),
    PageDown,
    PageUp,
    ScrollTop,
    ScrollBottom,
    ScrollLeft,
    ScrollRight,
    ScrollLineStart,
    NextHunk,
    PreviousHunk,
    ToggleHelp,
    ToggleDiagnostics,
    /// Open or close the history screen (recent local commits).
    ToggleHistory,
    CloseOverlay,
    ToggleSidebar,
    GrowSidebar,
    ShrinkSidebar,
    Refresh,
    Resize(u16, u16),
    Quit,
}
