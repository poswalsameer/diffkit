//! Interface robustness for unusual input: names with bidirectional and
//! zero-width characters, navigation keys on views without hunks, and
//! extremely long names at every width.

use crate::support;

use diffkit::{tui::actions::Action, types::diff::DiffComparison::*};
use support::{render_buffer, screen_text, tui_app, Repo, TempDir};

#[test]
fn bidi_and_zero_width_names_are_escaped_everywhere() {
    let tmp = TempDir::new("states-bidi");
    let repo = Repo::init_with_branch(tmp.join("repo"), "feature/\u{202e}txt.exe");
    repo.write("a.txt", "a\n");
    repo.commit_all("base");
    repo.write("in\u{200b}visible\u{2066}name.txt", "x\n");
    let app = tui_app(&repo.path, 140, 30);
    let text = screen_text(&app);
    assert!(text.contains("⎇ feature/\\u{202e}txt.exe"), "{text}");
    assert!(
        text.contains("in\\u{200b}visible\\u{2066}name.txt"),
        "{text}"
    );
    let buf = render_buffer(&app);
    assert!(
        !buf.content.iter().any(|c| c
            .symbol()
            .chars()
            .any(|ch| ch.is_control() || diffkit::helpers::display::is_format_control(ch))),
        "no control or format character reaches a cell"
    );
}

#[test]
fn navigation_on_views_without_hunks_is_harmless() {
    let tmp = TempDir::new("states-nohunks");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("c.txt", "base\n");
    repo.write_bytes("pic.bin", b"\x00\x01");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("c.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("c.txt", "main\n");
    repo.commit_all("main");
    repo.failing_merge("side");
    repo.write_bytes("pic.bin", b"\x00\x01\x02");
    let mut app = tui_app(&repo.path, 120, 30);
    app.dispatch(Action::SetComparison(Unstaged));
    for _ in 0..app.mode_state(Unstaged).files().len() {
        for a in [
            Action::NextHunk,
            Action::PreviousHunk,
            Action::ScrollBottom,
            Action::PageDown,
            Action::ScrollRight,
            Action::ScrollTop,
        ] {
            app.dispatch(a);
            assert_eq!(app.scroll().top, 0, "{a:?}: nothing to scroll");
            assert_eq!(app.scroll().hunk, None);
            let _ = screen_text(&app);
        }
        app.dispatch(Action::SelectNextFile);
    }
}

#[test]
fn very_long_names_and_subjects_fit_at_every_width() {
    let tmp = TempDir::new("states-long");
    let repo = Repo::init(tmp.join("repo"));
    let long = format!("{}/{}.txt", "deep".repeat(40), "名".repeat(60));
    repo.write("a.txt", "a\n");
    repo.commit_all(&"An extremely long subject line ".repeat(20));
    repo.write(&long, "x\n");
    let mut app = tui_app(&repo.path, 120, 30);
    app.dispatch(Action::SetComparison(Untracked));
    for (w, h) in [(40, 10), (60, 15), (90, 20), (250, 40)] {
        app.dispatch(Action::Resize(w, h));
        // The buffer is exactly w x h cells by construction; what matters is
        // that names are cut with "…" and the panel frames stay intact.
        let lines = support::screen(&app);
        assert_eq!(lines.len(), h as usize);
        let title = &lines[2];
        assert!(title.contains('…') && title.ends_with('┐'), "{w}: {title}");
        app.dispatch(Action::ToggleHistory);
        let lines = support::screen(&app);
        let row = lines
            .iter()
            .find(|l| l.contains("An ex"))
            .unwrap_or_else(|| panic!("{w}: {lines:#?}"));
        assert!(row.contains('…'), "{w}: {row}");
        assert!(lines[2].ends_with('┐'), "{w}: {}", lines[2]);
        app.dispatch(Action::ToggleHistory);
    }
}
