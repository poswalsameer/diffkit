//! TUI application state against real repositories: selection, comparison
//! modes, scrolling, hunks, overlays, refresh, staleness, caching, and the
//! guarantee that rendering and scrolling never run Git.

mod common;

use common::{numbered, render_buffer, screen_text, tui_app, Repo, TempDir};
use diffkit::{
    diff::{DiffComparison, DiffRowKind},
    git::commands::thread_invocations,
    tui::{
        app::{FailureKind, Loaded, Overlay, StatusKind},
        diff_view::{Body, ViewRow},
        Action, App,
    },
};
use std::path::Path;

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

fn selected_path(app: &App) -> Option<String> {
    app.selected_file()
        .map(|f| f.path().to_string_lossy().into_owned())
}

/// (kind, old number, new number, old text, new text) of a loaded row.
type RowText = (DiffRowKind, Option<usize>, Option<usize>, String, String);

fn rows(app: &App) -> Vec<RowText> {
    let Some(v) = app.view() else {
        return Vec::new();
    };
    let Body::Rows { rows, .. } = &v.body else {
        return Vec::new();
    };
    let text = |c: &Option<diffkit::tui::diff_view::Cell>| {
        c.as_ref()
            .map(|c| {
                c.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    rows.iter()
        .filter_map(|r| match r {
            ViewRow::Line { kind, old, new } => Some((
                *kind,
                old.as_ref().map(|c| c.number),
                new.as_ref().map(|c| c.number),
                text(old),
                text(new),
            )),
            ViewRow::Hunk { .. } => None,
        })
        .collect()
}

fn three_files(label: &str) -> (TempDir, Repo) {
    let (t, repo) = repo(label);
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, &numbered(&format!("{f} line "), 5));
    }
    repo.commit_all("base");
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(
            f,
            &numbered(&format!("{f} line "), 5).replace("line 3\n", "line three\n"),
        );
    }
    (t, repo)
}

#[test]
fn startup_discovers_everything_but_loads_only_the_selected_file() {
    let (_t, repo) = three_files("tui-startup");
    repo.write("staged.txt", "s\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("untracked.txt", "u\n");
    let before = thread_invocations();
    let mut app = App::open(&repo.path, ratatui::layout::Rect::new(0, 0, 120, 40)).unwrap();
    let opened = thread_invocations() - before;
    // Snapshot (rev-parse + status) + 4 discoveries; no diff loaded yet.
    assert_eq!(
        opened, 6,
        "rev-parse, status, staged, unstaged, combined, untracked"
    );
    assert_eq!(app.cached_entries(), 0);
    assert!(app.is_loading());
    assert_eq!(app.mode(), DiffComparison::Unstaged);
    assert_eq!(app.mode_state(DiffComparison::Unstaged).files().len(), 3);
    assert_eq!(app.mode_state(DiffComparison::Staged).files().len(), 1);
    assert_eq!(app.mode_state(DiffComparison::Untracked).files().len(), 1);
    let before = thread_invocations();
    app.load_pending();
    assert_eq!(thread_invocations() - before, 1, "exactly one diff loaded");
    assert_eq!(app.cached_entries(), 1);
    assert_eq!(selected_path(&app).as_deref(), Some("a.txt"));
}

#[test]
fn file_selection_moves_within_bounds() {
    let (_t, repo) = three_files("tui-select");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::SelectPreviousFile);
    assert_eq!(
        selected_path(&app).as_deref(),
        Some("a.txt"),
        "no wrap above the first"
    );
    app.dispatch(Action::SelectNextFile);
    assert_eq!(selected_path(&app).as_deref(), Some("b.txt"));
    assert!(rows(&app).iter().any(|r| r.4 == "b.txt line three"));
    app.dispatch(Action::SelectLastFile);
    app.dispatch(Action::SelectNextFile);
    assert_eq!(
        selected_path(&app).as_deref(),
        Some("c.txt"),
        "no wrap below the last"
    );
    app.dispatch(Action::SelectFirstFile);
    assert_eq!(selected_path(&app).as_deref(), Some("a.txt"));
    app.dispatch(Action::SidebarPageDown);
    assert_eq!(selected_path(&app).as_deref(), Some("c.txt"));
}

#[test]
fn empty_repository_changes_are_handled() {
    let (_t, repo) = repo("tui-empty");
    repo.write("a.txt", "a\n");
    repo.commit_all("base");
    let mut app = tui_app(&repo.path, 100, 30);
    for c in DiffComparison::ALL {
        assert!(app.mode_state(c).files().is_empty());
    }
    for a in [
        Action::SelectNextFile,
        Action::SelectLastFile,
        Action::NextHunk,
        Action::PageDown,
        Action::ScrollRight,
        Action::OpenSelected,
        Action::NextComparison,
    ] {
        app.dispatch(a);
    }
    assert!(app.selected_file().is_none());
    let text = screen_text(&app);
    assert_eq!(
        app.mode(),
        DiffComparison::Combined,
        "NextComparison from unstaged"
    );
    assert!(text.contains("No combined changes"), "{text}");
    assert!(text.contains("Nothing to show in this comparison."));
}

#[test]
fn comparison_modes_keep_partial_staging_distinct() {
    let (_t, repo) = repo("tui-modes");
    repo.write("f.txt", "line1\nvalue = A\nline3\n");
    repo.commit_all("A");
    repo.write("f.txt", "line1\nvalue = B\nline3\n");
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", "line1\nvalue = C\nline3\n");
    let mut app = tui_app(&repo.path, 120, 40);
    let modified = |app: &App| {
        rows(app)
            .into_iter()
            .find(|r| r.0 == DiffRowKind::Modified)
            .map(|r| (r.1, r.2, r.3, r.4))
    };
    assert_eq!(app.mode(), DiffComparison::Unstaged);
    assert_eq!(
        modified(&app),
        Some((Some(2), Some(2), "value = B".into(), "value = C".into()))
    );
    app.dispatch(Action::SetComparison(DiffComparison::Staged));
    assert_eq!(
        modified(&app),
        Some((Some(2), Some(2), "value = A".into(), "value = B".into()))
    );
    app.dispatch(Action::SetComparison(DiffComparison::Combined));
    assert_eq!(
        modified(&app),
        Some((Some(2), Some(2), "value = A".into(), "value = C".into()))
    );
    app.dispatch(Action::NextComparison);
    assert_eq!(app.mode(), DiffComparison::Untracked);
    app.dispatch(Action::NextComparison);
    assert_eq!(app.mode(), DiffComparison::Staged, "Tab wraps around");
    app.dispatch(Action::PreviousComparison);
    assert_eq!(app.mode(), DiffComparison::Untracked);
}

#[test]
fn selection_is_remembered_per_comparison() {
    let (_t, repo) = three_files("tui-mode-selection");
    repo.write("s1.txt", "1\n");
    repo.write("s2.txt", "2\n");
    repo.git(&["add", "s1.txt", "s2.txt"]);
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::SelectLastFile);
    app.dispatch(Action::SetComparison(DiffComparison::Staged));
    app.dispatch(Action::SelectNextFile);
    assert_eq!(selected_path(&app).as_deref(), Some("s2.txt"));
    app.dispatch(Action::SetComparison(DiffComparison::Unstaged));
    assert_eq!(selected_path(&app).as_deref(), Some("c.txt"));
    assert_eq!(app.scroll().top, 0, "diff scroll resets on mode switch");
}

#[test]
fn vertical_and_horizontal_scroll_are_bounded() {
    let (_t, repo) = repo("tui-scroll");
    repo.write("big.txt", &numbered("row ", 300));
    repo.commit_all("base");
    let long = format!("{}END\n", "x".repeat(400));
    repo.write("big.txt", &(numbered("new ", 300) + &long));
    let mut app = tui_app(&repo.path, 120, 40);
    let total = app.view().unwrap().row_count();
    app.dispatch(Action::ScrollBottom);
    let bottom = app.scroll().top;
    assert!(bottom > 0 && bottom < total);
    app.dispatch(Action::ScrollDown(1000));
    assert_eq!(app.scroll().top, bottom, "cannot scroll past the end");
    app.dispatch(Action::ScrollTop);
    assert_eq!(app.scroll().top, 0);
    app.dispatch(Action::ScrollUp(5));
    assert_eq!(app.scroll().top, 0);
    app.dispatch(Action::PageDown);
    assert!(app.scroll().top > 10);
    app.dispatch(Action::PageUp);
    assert_eq!(app.scroll().top, 0);
    for _ in 0..200 {
        app.dispatch(Action::ScrollRight);
    }
    let left = app.scroll().left;
    let max = app.view().unwrap().max_width();
    assert!(left > 0 && left < max, "{left} {max}");
    app.dispatch(Action::ScrollRight);
    assert_eq!(
        app.scroll().left,
        left,
        "cannot scroll past the widest line"
    );
    app.dispatch(Action::ScrollLineStart);
    assert_eq!(app.scroll().left, 0);
}

#[test]
fn hunk_navigation() {
    let (_t, repo) = repo("tui-hunks");
    repo.write("f.txt", &numbered("l", 100));
    repo.commit_all("base");
    repo.write(
        "f.txt",
        &numbered("l", 100)
            .replace("l10\n", "ten\n")
            .replace("l50\n", "fifty\n")
            .replace("l90\n", "ninety\n"),
    );
    let mut app = tui_app(&repo.path, 120, 20);
    let starts = app.view().unwrap().hunk_starts().to_vec();
    assert_eq!(starts.len(), 3);
    assert_eq!(app.scroll().hunk, Some(0));
    app.dispatch(Action::NextHunk);
    assert_eq!((app.scroll().hunk, app.scroll().top), (Some(1), starts[1]));
    app.dispatch(Action::NextHunk);
    assert_eq!(app.scroll().hunk, Some(2));
    app.dispatch(Action::NextHunk);
    assert_eq!(app.scroll().hunk, Some(2), "stays on the last hunk");
    app.dispatch(Action::PreviousHunk);
    assert_eq!((app.scroll().hunk, app.scroll().top), (Some(1), starts[1]));
    app.dispatch(Action::PreviousHunk);
    app.dispatch(Action::PreviousHunk);
    assert_eq!((app.scroll().hunk, app.scroll().top), (Some(0), 0));
    let text = screen_text(&app);
    assert!(
        text.contains("▶ @@ -7,7 +7,7 @@"),
        "selected hunk is marked:\n{text}"
    );
}

#[test]
fn hunk_navigation_without_hunks_is_a_no_op() {
    let (_t, repo) = repo("tui-no-hunks");
    std::fs::write(repo.path.join("b.bin"), b"\x00a").unwrap();
    repo.commit_all("base");
    std::fs::write(repo.path.join("b.bin"), b"\x00b").unwrap();
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::NextHunk);
    app.dispatch(Action::PreviousHunk);
    app.dispatch(Action::ScrollBottom);
    assert_eq!(app.scroll().top, 0);
    assert_eq!(app.scroll().hunk, None);
}

#[test]
fn help_overlay_and_status_dismissal() {
    let (_t, repo) = three_files("tui-help");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::ToggleHelp);
    assert_eq!(app.overlay(), Some(&Overlay::Help));
    assert!(screen_text(&app).contains("Help — ? or Esc to close"));
    app.dispatch(Action::ToggleHelp);
    assert_eq!(app.overlay(), None);
    app.dispatch(Action::ToggleHelp);
    app.dispatch(Action::CloseOverlay);
    assert_eq!(app.overlay(), None);
    app.dispatch(Action::Refresh);
    assert_eq!(app.status().unwrap().kind, StatusKind::Info);
    app.dispatch(Action::CloseOverlay);
    assert!(app.status().is_none(), "Esc clears the message");
    app.dispatch(Action::Quit);
    assert!(app.should_quit());
}

#[test]
fn refresh_keeps_selection_by_identity() {
    let (_t, repo) = three_files("tui-refresh");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::SelectNextFile);
    app.dispatch(Action::SelectNextFile);
    assert_eq!(selected_path(&app).as_deref(), Some("c.txt"));
    // An external tool changes the repository: a file sorting before c.txt
    // now has changes too, shifting indices.
    repo.write("aa.txt", "new tracked\n");
    repo.git(&["add", "aa.txt"]);
    repo.git(&["commit", "-q", "-m", "aa"]);
    repo.write("aa.txt", "changed\n");
    app.dispatch(Action::Refresh);
    assert_eq!(app.mode_state(DiffComparison::Unstaged).files().len(), 4);
    assert_eq!(
        selected_path(&app).as_deref(),
        Some("c.txt"),
        "same file after refresh"
    );
    assert_eq!(app.mode_state(DiffComparison::Unstaged).selected, 3);
    assert!(rows(&app).iter().any(|r| r.4 == "c.txt line three"));
    assert_eq!(app.cached_entries(), 1, "cache rebuilt from scratch");
}

#[test]
fn refresh_picks_up_external_staging() {
    let (_t, repo) = three_files("tui-refresh-stage");
    let mut app = tui_app(&repo.path, 120, 40);
    assert!(app.mode_state(DiffComparison::Staged).files().is_empty());
    repo.git(&["add", "b.txt"]);
    app.dispatch(Action::Refresh);
    let staged: Vec<_> = app
        .mode_state(DiffComparison::Staged)
        .files()
        .iter()
        .map(|f| f.path().to_owned())
        .collect();
    assert_eq!(staged, vec![Path::new("b.txt")]);
    assert_eq!(app.mode_state(DiffComparison::Unstaged).files().len(), 2);
}

#[test]
fn stale_file_is_reported_not_shown() {
    let (_t, repo) = three_files("tui-stale");
    let mut app = tui_app(&repo.path, 120, 40);
    // b.txt changes after discovery but before it is loaded.
    std::thread::sleep(std::time::Duration::from_millis(20));
    repo.write("b.txt", "completely different content now\n");
    app.dispatch(Action::SelectNextFile);
    match app.loaded() {
        Some(Loaded::Failed(f)) => {
            assert_eq!(f.kind, FailureKind::Stale);
            assert!(f.message.contains("Press r to refresh"), "{}", f.message);
        }
        other => panic!("{other:?}"),
    }
    assert!(app.view().is_none(), "no outdated content is shown");
    assert_eq!(app.status().unwrap().kind, StatusKind::Warning);
    let text = screen_text(&app);
    assert!(
        text.contains("This file changed since the list was loaded"),
        "{text}"
    );
    app.dispatch(Action::Refresh);
    assert!(rows(&app)
        .iter()
        .any(|r| r.4 == "completely different content now"));
}

#[test]
fn returning_to_a_loaded_file_uses_the_cache() {
    let (_t, repo) = three_files("tui-cache");
    let mut app = tui_app(&repo.path, 120, 40);
    app.dispatch(Action::SelectNextFile);
    let before = thread_invocations();
    app.dispatch(Action::SelectPreviousFile);
    assert_eq!(thread_invocations(), before, "a.txt comes from the cache");
    app.dispatch(Action::OpenSelected);
    assert_eq!(thread_invocations(), before + 1, "Enter reloads explicitly");
}

#[test]
fn rendering_scrolling_and_resizing_never_run_git() {
    let (_t, repo) = repo("tui-no-git");
    repo.write("f.txt", &numbered("l", 200));
    repo.commit_all("base");
    repo.write("f.txt", &numbered("l", 200).replace("l100\n", "hundred\n"));
    repo.write("g.txt", "untracked\n");
    let mut app = tui_app(&repo.path, 120, 40);
    let before = thread_invocations();
    for a in [
        Action::ScrollDown(3),
        Action::PageDown,
        Action::ScrollBottom,
        Action::ScrollTop,
        Action::NextHunk,
        Action::PreviousHunk,
        Action::ScrollRight,
        Action::ScrollLeft,
        Action::ToggleSidebar,
        Action::ToggleSidebar,
        Action::GrowSidebar,
        Action::ShrinkSidebar,
        Action::ToggleHelp,
        Action::CloseOverlay,
        Action::Resize(80, 24),
        Action::Resize(200, 60),
        Action::Resize(30, 8),
        Action::Resize(120, 40),
    ] {
        app.dispatch(a);
        render_buffer(&app);
    }
    assert_eq!(thread_invocations(), before);
}

#[test]
fn resizing_keeps_scroll_valid() {
    let (_t, repo) = repo("tui-resize");
    repo.write("f.txt", &numbered("l", 120));
    repo.commit_all("base");
    repo.write("f.txt", &numbered("n", 120));
    let mut app = tui_app(&repo.path, 120, 60);
    app.dispatch(Action::ScrollBottom);
    let total = app.view().unwrap().row_count();
    for (w, h) in [(80, 24), (200, 100), (40, 10), (10, 3), (120, 40)] {
        app.dispatch(Action::Resize(w, h));
        assert!(app.scroll().top < total, "{w}x{h}");
        render_buffer(&app);
    }
    app.dispatch(Action::Resize(200, 300));
    assert_eq!(app.scroll().top, 0, "everything fits: no scroll left");
}

#[test]
fn sidebar_can_be_hidden_and_resized() {
    let (_t, repo) = three_files("tui-sidebar");
    let mut app = tui_app(&repo.path, 120, 40);
    let w = app.sidebar().width;
    app.dispatch(Action::GrowSidebar);
    assert_eq!(app.sidebar().width, w + 4);
    for _ in 0..50 {
        app.dispatch(Action::GrowSidebar);
    }
    assert_eq!(app.sidebar().width, 48, "at most 40% of 120 columns");
    for _ in 0..50 {
        app.dispatch(Action::ShrinkSidebar);
    }
    assert_eq!(app.sidebar().width, 16);
    app.dispatch(Action::ToggleSidebar);
    assert!(!app.sidebar().visible);
    assert!(!screen_text(&app).contains("CHANGES"));
}

#[test]
fn full_flow_over_every_kind_of_change() {
    let (_t, repo) = repo("tui-flow");
    let body = numbered("shared ", 20);
    repo.write("to-rename.txt", &body);
    repo.write("conflict.txt", "base\n");
    std::fs::write(repo.path.join("image.bin"), b"\x00old").unwrap();
    repo.write("doomed.txt", "bye\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.commit_all("main");
    assert!(!repo.git_output(&["merge", "side"]).status.success());
    repo.git(&["mv", "to-rename.txt", "renamed.txt"]);
    std::fs::write(repo.path.join("image.bin"), b"\x00new!").unwrap();
    repo.git(&["rm", "-q", "doomed.txt"]);
    repo.write("brand-new.txt", "fresh\n");
    let mut app = tui_app(&repo.path, 140, 45);

    let mut seen = Vec::new();
    for c in DiffComparison::ALL {
        app.dispatch(Action::SetComparison(c));
        let n = app.mode_state(c).files().len();
        for i in 0..n {
            if i > 0 {
                app.dispatch(Action::SelectNextFile);
            }
            let path = selected_path(&app).unwrap();
            let loaded = app.loaded().expect("loaded on selection");
            let text = screen_text(&app);
            match loaded {
                Loaded::View(v) => {
                    if let Body::Notice(diffkit::tui::diff_view::Notice::Binary(_)) = v.body {
                        assert!(text.contains("Binary file — content preview unavailable."));
                    }
                    if let Body::Notice(diffkit::tui::diff_view::Notice::Unmerged) = v.body {
                        assert!(text.contains("CONFLICT"), "{text}");
                    }
                }
                Loaded::Failed(f) => panic!("{c:?} {path}: {}", f.message),
            }
            seen.push((c, path));
        }
    }
    let has = |c, p: &str| seen.iter().any(|(sc, sp)| *sc == c && sp == p);
    assert!(has(DiffComparison::Staged, "renamed.txt"));
    assert!(has(DiffComparison::Staged, "doomed.txt"));
    assert!(has(DiffComparison::Staged, "conflict.txt"));
    assert!(has(DiffComparison::Unstaged, "image.bin"));
    assert!(has(DiffComparison::Unstaged, "conflict.txt"));
    assert!(has(DiffComparison::Combined, "conflict.txt"));
    assert!(has(DiffComparison::Untracked, "brand-new.txt"));
    app.dispatch(Action::Refresh);
    assert_eq!(app.mode(), DiffComparison::Untracked);
    assert_eq!(selected_path(&app).as_deref(), Some("brand-new.txt"));
}
