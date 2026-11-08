//! The history screen: listing recent commits, opening one, viewing its
//! diffs, and returning to an unchanged working-tree view, including while
//! synchronization results arrive.

use crate::support;

use diffkit::{
    sync::{events::ChangeSet, reconcile},
    tui::{actions::Action, app::App},
    types::diff::{DiffComparison, DiffOptions},
};
use std::path::Path;
use support::{assert_read_only, git, numbered, screen_text, tui_app, Repo, TempDir};

/// Three commits (the last touching two files) and working-tree changes
/// in two files, one long enough to scroll.
fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", &numbered("a ", 200));
    repo.write("b.txt", "b\n");
    repo.commit_all("first commit");
    repo.write("c.txt", "c\n");
    repo.commit_all("second commit");
    repo.write(
        "a.txt",
        &numbered("a ", 200).replace("a 150\n", "a one-fifty\n"),
    );
    repo.write("b.txt", "b\nmore\n");
    repo.commit_all("third commit");
    repo.write("a.txt", &numbered("A ", 200));
    repo.write("b.txt", "b\nmore\nworking tree\n");
    (tmp, repo)
}

fn selected(app: &App) -> String {
    app.selected_file().unwrap().path().display().to_string()
}

#[test]
fn inspect_a_commit_and_return_to_an_unchanged_working_tree_view() {
    let (_t, repo) = repo("hist-tui");
    assert_read_only(&repo.path, || {
        let mut app = tui_app(&repo.path, 140, 40);
        app.dispatch(Action::SetComparison(DiffComparison::Unstaged));
        app.dispatch(Action::SelectFirstFile);
        app.dispatch(Action::ScrollDown(30));
        let before = (app.mode(), selected(&app), app.scroll());
        assert!(before.2.top > 0);

        app.dispatch(Action::ToggleHistory);
        let text = screen_text(&app);
        assert!(text.contains(" HISTORY (3) "), "{text}");
        assert!(
            text.contains("third commit") && text.contains("first commit"),
            "{text}"
        );
        assert!(text.contains("recent commits reachable from HEAD (local history only)"));
        assert!(
            text.contains("commit    "),
            "metadata of the selected commit: {text}"
        );
        assert!(text.contains("commit 1/3"));

        // Open the newest commit: its two files, the first loaded.
        app.dispatch(Action::OpenSelected);
        let text = screen_text(&app);
        assert!(text.contains(" COMMIT "), "{text}");
        assert!(text.contains("FILES IN"), "{text}");
        assert!(
            text.contains("OLD · parent") && text.contains("NEW · commit"),
            "{text}"
        );
        assert!(text.contains("file 1/2"), "{text}");
        assert_eq!(app.scroll().top, 0, "the commit view has its own scroll");
        app.dispatch(Action::NextHunk);
        let text = screen_text(&app);
        assert!(text.contains("a one-fifty"), "{text}");
        app.dispatch(Action::SelectNextFile);
        assert!(screen_text(&app).contains("file 2/2"));

        // Back to the list, then to the working tree exactly as it was.
        app.dispatch(Action::CloseOverlay);
        assert!(app.history().is_some_and(|h| h.open.is_none()));
        app.dispatch(Action::CloseOverlay);
        assert!(app.history().is_none());
        assert_eq!((app.mode(), selected(&app), app.scroll()), before);
        assert!(screen_text(&app).contains("CHANGES (2)"));
    });
}

#[test]
fn synchronization_while_history_is_open_updates_only_the_working_tree() {
    let (_t, repo) = repo("hist-sync");
    let mut app = tui_app(&repo.path, 140, 40);
    app.dispatch(Action::SetComparison(DiffComparison::Unstaged));
    app.dispatch(Action::ToggleHistory);
    app.dispatch(Action::SelectNextFile);
    app.dispatch(Action::OpenSelected);
    let opened = app
        .history()
        .unwrap()
        .open
        .as_ref()
        .unwrap()
        .commit
        .id
        .clone();
    let shown = screen_text(&app);

    repo.write("d.txt", "new while inspecting\n");
    repo.git(&["add", "d.txt"]);
    let r =
        reconcile::reconcile(&repo.path, DiffOptions::default(), ChangeSet::INDEX, None).unwrap();
    app.apply_reconciled(r);
    app.load_pending();
    let h = app.history().unwrap();
    assert_eq!(h.open.as_ref().unwrap().commit.id, opened);
    let after = screen_text(&app);
    let body = |s: &str| s.lines().skip(2).take(30).collect::<Vec<_>>().join("\n");
    assert_eq!(body(&after), body(&shown), "the commit view is untouched");

    app.dispatch(Action::ToggleHistory);
    app.dispatch(Action::SetComparison(DiffComparison::Staged));
    assert!(
        screen_text(&app).contains("d.txt"),
        "the working tree caught up"
    );
}

#[test]
fn comparison_keys_leave_history_and_switch() {
    let (_t, repo) = repo("hist-keys");
    let mut app = tui_app(&repo.path, 120, 30);
    app.dispatch(Action::ToggleHistory);
    app.dispatch(Action::OpenSelected);
    app.dispatch(Action::SetComparison(DiffComparison::Combined));
    assert!(app.history().is_none());
    assert_eq!(app.mode(), DiffComparison::Combined);
    app.dispatch(Action::ToggleHistory);
    app.dispatch(Action::ToggleHistory);
    assert!(app.history().is_none(), "L toggles");
}

#[test]
fn empty_merge_and_shallow_boundary_commits_are_explained() {
    let tmp = TempDir::new("hist-special");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "a\n");
    repo.commit_all("root");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("s.txt", "s\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("m.txt", "m\n");
    repo.commit_all("main");
    repo.git(&["merge", "-q", "--no-ff", "--no-edit", "side"]);
    repo.empty_commit("nothing changed");

    let mut app = tui_app(&repo.path, 140, 40);
    app.dispatch(Action::ToggleHistory);
    app.dispatch(Action::OpenSelected);
    let text = screen_text(&app);
    assert!(text.contains("This commit changes no files"), "{text}");
    app.dispatch(Action::CloseOverlay);
    app.dispatch(Action::SelectNextFile);
    let text = screen_text(&app);
    assert!(
        text.contains("(merge; diffs use the first parent)"),
        "{text}"
    );
    app.dispatch(Action::OpenSelected);
    let text = screen_text(&app);
    assert!(text.contains("OLD · first parent"), "{text}");
    assert!(text.contains("s.txt") && !text.contains("m.txt"), "{text}");

    // A shallow clone's boundary commit cannot be diffed.
    let url = format!("file://{}", repo.path.display());
    git(
        tmp.path(),
        &["clone", "-q", "--depth", "1", &url, "shallow"],
    );
    let shallow = support::real(&tmp.join("shallow"));
    let mut app = tui_app(&shallow, 140, 40);
    app.dispatch(Action::ToggleHistory);
    let text = screen_text(&app);
    assert!(
        text.contains("not available (shallow clone boundary)"),
        "{text}"
    );
    assert!(text.contains("Shallow clone: older history is not available locally."));
    app.dispatch(Action::OpenSelected);
    let text = screen_text(&app);
    assert!(text.contains("in this shallow clone, so the"), "{text}");
    assert!(text.contains("The files of this commit could not be listed."));
    assert!(
        !text.contains("Press r"),
        "reloading cannot fix this: {text}"
    );
}

#[test]
fn commit_text_is_escaped() {
    let tmp = TempDir::new("hist-escape");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "a\n");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "-c",
        "user.name=Evil\u{1b}]0;pwned\u{7}Name",
        "commit",
        "-q",
        "--cleanup=verbatim",
        "-m",
        "subject \u{1b}[2J\u{202e}gnp.exe",
    ]);
    let mut app = tui_app(&repo.path, 140, 40);
    app.dispatch(Action::ToggleHistory);
    let text = screen_text(&app);
    assert!(!text.contains('\u{1b}') && !text.contains('\u{202e}') && !text.contains('\u{7}'));
    assert!(
        text.contains("subject \\u{1b}[2J\\u{202e}gnp.exe"),
        "{text}"
    );
    assert!(text.contains("Evil\\u{1b}]0;pwned\\u{7}Name"), "{text}");
}

#[test]
fn history_renders_at_tiny_sizes_and_reloads() {
    let (_t, repo) = repo("hist-small");
    let mut app = tui_app(&repo.path, 120, 30);
    app.dispatch(Action::ToggleHistory);
    for (w, h) in [(40, 10), (30, 6), (200, 12), (41, 50)] {
        app.dispatch(Action::Resize(w, h));
        let _ = screen_text(&app);
        app.dispatch(Action::OpenSelected);
        let _ = screen_text(&app);
        app.dispatch(Action::ToggleSidebar);
        let _ = screen_text(&app);
        app.dispatch(Action::CloseOverlay);
    }
    app.dispatch(Action::Resize(120, 30));
    repo.commit_all("fourth commit");
    app.dispatch(Action::Refresh);
    let text = screen_text(&app);
    assert!(text.contains("fourth commit"), "{text}");
    assert!(text.contains("History reloaded: 4 commits"), "{text}");
    assert!(Path::new(&repo.path).exists());
}
