//! Keys to actions. Kept separate from state so the mapping is testable and
//! documented in one place (see also `ui::overlays::KEYS`).

use super::{actions::Action, app::Overlay};
use crate::types::diff::DiffComparison;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub fn action_for(key: KeyEvent, overlay: Option<&Overlay>) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if ctrl {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            KeyCode::Char('d') if overlay.is_none() => Some(Action::SidebarPageDown),
            KeyCode::Char('u') if overlay.is_none() => Some(Action::SidebarPageUp),
            _ => None,
        };
    }
    if alt {
        return None;
    }
    match overlay {
        Some(Overlay::Help) => {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('?') => Some(Action::CloseOverlay),
                KeyCode::Char('q') => Some(Action::Quit),
                _ => None,
            }
        }
        Some(Overlay::Diagnostics) => {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('D') => Some(Action::CloseOverlay),
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Char('r') => Some(Action::Refresh),
                _ => None,
            }
        }
        Some(Overlay::Error { .. }) => {
            return match key.code {
                KeyCode::Esc | KeyCode::Enter => Some(Action::CloseOverlay),
                KeyCode::Char('r') => Some(Action::Refresh),
                KeyCode::Char('q') => Some(Action::Quit),
                _ => None,
            }
        }
        None => {}
    }
    Some(match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('D') => Action::ToggleDiagnostics,
        KeyCode::Char('L') => Action::ToggleHistory,
        KeyCode::Char('j') | KeyCode::Down => Action::SelectNextFile,
        KeyCode::Char('k') | KeyCode::Up => Action::SelectPreviousFile,
        KeyCode::Home => Action::SelectFirstFile,
        KeyCode::End => Action::SelectLastFile,
        KeyCode::Enter => Action::OpenSelected,
        KeyCode::Char('1') => Action::SetComparison(DiffComparison::Staged),
        KeyCode::Char('2') => Action::SetComparison(DiffComparison::Unstaged),
        KeyCode::Char('3') => Action::SetComparison(DiffComparison::Combined),
        KeyCode::Char('4') => Action::SetComparison(DiffComparison::Untracked),
        KeyCode::Tab => Action::NextComparison,
        KeyCode::BackTab => Action::PreviousComparison,
        KeyCode::Char('J') => Action::ScrollDown(1),
        KeyCode::Char('K') => Action::ScrollUp(1),
        KeyCode::PageDown | KeyCode::Char(' ') => Action::PageDown,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::Char('g') => Action::ScrollTop,
        KeyCode::Char('G') => Action::ScrollBottom,
        KeyCode::Char('l') | KeyCode::Right => Action::ScrollRight,
        KeyCode::Char('h') | KeyCode::Left => Action::ScrollLeft,
        KeyCode::Char('0') => Action::ScrollLineStart,
        KeyCode::Char(']') => Action::NextHunk,
        KeyCode::Char('[') => Action::PreviousHunk,
        KeyCode::Char('b') => Action::ToggleSidebar,
        KeyCode::Char('<') => Action::ShrinkSidebar,
        KeyCode::Char('>') => Action::GrowSidebar,
        KeyCode::Char('r') => Action::Refresh,
        _ => return None,
    })
}
