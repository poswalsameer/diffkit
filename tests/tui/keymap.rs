//! Key bindings: every documented key, modifiers and key releases that must
//! not alias, and overlays that capture keys.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use diffkit::{
    tui::{actions::Action, app::Overlay, keymap::*},
    types::diff::DiffComparison,
};

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
        (
            key(KeyCode::Char('D'), KeyModifiers::SHIFT),
            Action::ToggleDiagnostics,
        ),
        (
            key(KeyCode::Char('L'), KeyModifiers::SHIFT),
            Action::ToggleHistory,
        ),
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
