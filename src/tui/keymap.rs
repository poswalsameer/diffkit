//! Keys to actions. Kept separate from state so the mapping is testable and
//! documented in one place (see also `ui::overlays::KEYS`).

use super::{actions::Action, app::Overlay};
use crate::diff::DiffComparison;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn plain(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn documented_bindings() {
        let cases = [
            (plain('j'), Action::SelectNextFile),
            (
                key(KeyCode::Down, KeyModifiers::NONE),
                Action::SelectNextFile,
            ),
            (plain('k'), Action::SelectPreviousFile),
            (
                key(KeyCode::Enter, KeyModifiers::NONE),
                Action::OpenSelected,
            ),
            (plain('1'), Action::SetComparison(DiffComparison::Staged)),
            (plain('4'), Action::SetComparison(DiffComparison::Untracked)),
            (
                key(KeyCode::Tab, KeyModifiers::NONE),
                Action::NextComparison,
            ),
            (
                key(KeyCode::BackTab, KeyModifiers::SHIFT),
                Action::PreviousComparison,
            ),
            (
                key(KeyCode::Char('d'), KeyModifiers::CONTROL),
                Action::SidebarPageDown,
            ),
            (
                key(KeyCode::Char('u'), KeyModifiers::CONTROL),
                Action::SidebarPageUp,
            ),
            (
                key(KeyCode::Char('J'), KeyModifiers::SHIFT),
                Action::ScrollDown(1),
            ),
            (
                key(KeyCode::Char('K'), KeyModifiers::SHIFT),
                Action::ScrollUp(1),
            ),
            (key(KeyCode::PageDown, KeyModifiers::NONE), Action::PageDown),
            (plain('g'), Action::ScrollTop),
            (
                key(KeyCode::Char('G'), KeyModifiers::SHIFT),
                Action::ScrollBottom,
            ),
            (plain(']'), Action::NextHunk),
            (plain('['), Action::PreviousHunk),
            (plain('r'), Action::Refresh),
            (plain('?'), Action::ToggleHelp),
            (key(KeyCode::Esc, KeyModifiers::NONE), Action::CloseOverlay),
            (plain('q'), Action::Quit),
            (key(KeyCode::Char('c'), KeyModifiers::CONTROL), Action::Quit),
        ];
        for (k, a) in cases {
            assert_eq!(action_for(k, None), Some(a), "{k:?}");
        }
    }

    #[test]
    fn modifiers_and_releases_do_not_alias_plain_keys() {
        assert_eq!(
            action_for(key(KeyCode::Char('j'), KeyModifiers::CONTROL), None),
            None
        );
        assert_eq!(
            action_for(key(KeyCode::Char('j'), KeyModifiers::ALT), None),
            None
        );
        let mut release = plain('j');
        release.kind = KeyEventKind::Release;
        assert_eq!(action_for(release, None), None);
        assert_eq!(action_for(plain('x'), None), None);
    }

    #[test]
    fn overlays_capture_keys() {
        let help = Overlay::Help;
        assert_eq!(action_for(plain('j'), Some(&help)), None);
        assert_eq!(
            action_for(plain('?'), Some(&help)),
            Some(Action::CloseOverlay)
        );
        assert_eq!(action_for(plain('q'), Some(&help)), Some(Action::Quit));
        let err = Overlay::Error {
            title: "t".into(),
            message: "m".into(),
        };
        assert_eq!(action_for(plain('r'), Some(&err)), Some(Action::Refresh));
        assert_eq!(
            action_for(key(KeyCode::Enter, KeyModifiers::NONE), Some(&err)),
            Some(Action::CloseOverlay)
        );
    }
}
