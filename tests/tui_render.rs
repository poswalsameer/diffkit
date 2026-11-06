//! Rendering through Ratatui's TestBackend: layout at several sizes,
//! side-by-side rows, special files, escaping, and colors.

mod common;

use common::{numbered, render_buffer, screen, screen_text, tui_app, Repo, TempDir};
use diffkit::{diff::DiffComparison, tui::Action};
use ratatui::style::Color;

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

/// A repository whose unstaged `src/app.js` has a modification, an
/// insertion, and context.
fn sample(label: &str) -> (TempDir, Repo) {
    let (t, repo) = repo(label);
    repo.write(
        "src/app.js",
        "const name = \"Alice\";\nconst age = 20;\nconsole.log(name);\n",
    );
    repo.write("zzz.txt", "z\n");
    repo.commit_all("base");
    repo.write(
        "src/app.js",
        "const name = \"Alice\";\nconst age = 21;\nconsole.log(name);\nconsole.log(age);\n",
    );
    repo.write("zzz.txt", "zz\n");
    (t, repo)
}

/// Row index and column (in cells) of the first occurrence of `needle`.
fn find(lines: &[String], needle: &str) -> Option<(usize, usize)> {
    lines
        .iter()
        .enumerate()
        .find_map(|(y, l)| l.find(needle).map(|byte| (y, l[..byte].chars().count())))
}

#[test]
fn normal_layout_shows_every_region() {
    let (_t, repo) = sample("render-normal");
    let app = tui_app(&repo.path, 120, 40);
    let lines = screen(&app);
    assert!(
        lines[0].starts_with(" DIFFKIT  repo │ ⎇ main (no upstream)"),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains("2 UNSTAGED 2") && lines[1].contains("index -> working tree"));
    assert!(lines[2].contains("CHANGES (2)") && lines[2].contains("src/app.js"));
    let text = lines.join("\n");
    for needle in [
        "M  src/app.js",
        "M  zzz.txt",
        "OLD · index",
        "NEW · working tree",
        "▶ @@ -1,3 +1,4 @@",
    ] {
        assert!(text.contains(needle), "missing {needle:?}:\n{text}");
    }
    assert!(
        lines[38].starts_with("file 1/2 · hunk 1/1 · rows 1-5 of 5"),
        "{}",
        lines[38]
    );
    assert!(lines[39].contains("? help") && lines[39].contains("q quit"));
}

#[test]
fn side_by_side_rows_numbers_markers_and_colors() {
    let (_t, repo) = sample("render-rows");
    let app = tui_app(&repo.path, 120, 40);
    let buf = render_buffer(&app);
    let lines = screen(&app);
    // Context on both sides with both numbers.
    let (y, _) = find(&lines, "const name = \"Alice\";").unwrap();
    assert_eq!(lines[y].matches("const name = \"Alice\";").count(), 2);
    assert!(lines[y].contains("  1   const name") && lines[y].matches("  1   ").count() == 2);
    // A paired modification: `~` on both sides, old red, new green.
    let (y, old_x) = find(&lines, "const age = 20;").unwrap();
    let (_, new_x) = find(&lines[y..=y], "const age = 21;").unwrap();
    assert!(lines[y].contains("2 ~ const age = 20;") && lines[y].contains("2 ~ const age = 21;"));
    assert_eq!(buf[(old_x as u16, y as u16)].fg, Color::Red);
    assert_eq!(buf[(new_x as u16, y as u16)].fg, Color::Green);
    // An insertion: nothing on the old side, `+` and number 4 on the new.
    let (y, x) = find(&lines, "console.log(age);").unwrap();
    assert!(lines[y].contains("4 + console.log(age);"));
    let screen_layout =
        diffkit::tui::layout::screen(app.size(), app.sidebar().width, true).unwrap();
    let areas = diffkit::tui::layout::diff_areas(screen_layout.diff, 3).unwrap();
    let old_side: String = lines[y]
        .chars()
        .skip(areas.old_gutter.x as usize)
        .take((areas.separator.x - areas.old_gutter.x) as usize)
        .collect();
    assert!(
        old_side.trim().is_empty(),
        "old side is empty: {old_side:?}"
    );
    assert_eq!(buf[(x as u16, y as u16)].fg, Color::Green);
}

#[test]
fn added_and_deleted_files_use_one_side() {
    let (_t, repo) = repo("render-add-delete");
    repo.write("gone.txt", "old one\nold two\n");
    repo.commit_all("base");
    repo.git(&["rm", "-q", "gone.txt"]);
    repo.write("new.txt", "new one\n");
    repo.git(&["add", "new.txt"]);
    let mut app = tui_app(&repo.path, 120, 30);
    assert_eq!(app.mode(), DiffComparison::Staged);
    let lines = screen(&app);
    assert!(
        lines.iter().any(|l| l.contains("1 - old one")),
        "deleted: old side only"
    );
    assert!(!lines.iter().any(|l| l.contains("+ old one")));
    app.dispatch(Action::SelectNextFile);
    let lines = screen(&app);
    let (y, _) = find(&lines, "new one").unwrap();
    assert!(lines[y].contains("1 + new one"));
    assert_eq!(
        lines[y].matches("new one").count(),
        1,
        "added: new side only"
    );
}

#[test]
fn no_newline_and_crlf_markers() {
    let (_t, repo) = repo("render-eol");
    std::fs::write(repo.path.join("f.txt"), b"one\ntwo").unwrap();
    std::fs::write(repo.path.join("w.txt"), b"alpha\r\n").unwrap();
    repo.commit_all("base");
    std::fs::write(repo.path.join("f.txt"), b"one\ntwo changed").unwrap();
    std::fs::write(repo.path.join("w.txt"), b"alpha beta\r\n").unwrap();
    let mut app = tui_app(&repo.path, 120, 30);
    let text = screen_text(&app);
    assert!(
        text.contains("two \\ no newline") && text.contains("two changed \\ no newline"),
        "{text}"
    );
    app.dispatch(Action::SelectNextFile);
    assert!(screen_text(&app).contains("alpha beta\u{240d}"));
}

#[test]
fn binary_conflict_mode_rename_and_empty_notices() {
    let (_t, repo) = repo("render-notices");
    std::fs::write(repo.path.join("pic.bin"), b"\x00abc").unwrap();
    repo.write("run.sh", "echo\n");
    repo.write("old name.txt", &numbered("shared ", 10));
    repo.write("c.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("c.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("c.txt", "main\n");
    repo.commit_all("main");
    assert!(!repo.git_output(&["merge", "side"]).status.success());
    std::fs::write(repo.path.join("pic.bin"), b"\x00abcdef").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.path.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    repo.write("empty.txt", "");

    let mut app = tui_app(&repo.path, 130, 40);
    let show = |app: &mut diffkit::tui::App, c, path: &str| {
        app.dispatch(Action::SetComparison(c));
        app.dispatch(Action::SelectFirstFile);
        while app
            .selected_file()
            .map(|f| f.path().to_string_lossy().into_owned())
            .as_deref()
            != Some(path)
        {
            let before = app.mode_state(c).selected;
            app.dispatch(Action::SelectNextFile);
            assert_ne!(app.mode_state(c).selected, before, "{path} not in {c:?}");
        }
        screen_text(app)
    };
    let t = show(&mut app, DiffComparison::Unstaged, "pic.bin");
    assert!(
        t.contains("Binary file — content preview unavailable.")
            && t.contains("Size: 4 bytes → 7 bytes"),
        "{t}"
    );
    let t = show(&mut app, DiffComparison::Unstaged, "c.txt");
    assert!(t.contains("⚠ CONFLICT — this path is unmerged"), "{t}");
    assert!(t.contains("U  c.txt"));
    let t = show(&mut app, DiffComparison::Combined, "c.txt");
    assert!(
        t.contains(" ⚠ CONFLICT  Modified") && t.contains("<<<<<<<"),
        "{t}"
    );
    #[cfg(unix)]
    {
        let t = show(&mut app, DiffComparison::Unstaged, "run.sh");
        assert!(
            t.contains("Only the file mode changed: 100644 → 100755.") && t.contains("X  run.sh"),
            "{t}"
        );
    }
    let t = show(&mut app, DiffComparison::Staged, "new name.txt");
    // The sidebar truncates from the start; the panel title has both names.
    assert!(
        t.contains("→ new name.txt") && t.contains("┌ old name.txt → new name.txt "),
        "{t}"
    );
    assert!(
        t.contains("Renamed · was old name.txt (100% similar)")
            && t.contains("Renamed without content changes.")
    );
    let t = show(&mut app, DiffComparison::Untracked, "empty.txt");
    assert!(
        t.contains("Empty file (no lines).") && t.contains("?? empty.txt"),
        "{t}"
    );
}

#[test]
fn standard_80_by_24_terminal() {
    let (_t, repo) = sample("render-80x24");
    let app = tui_app(&repo.path, 80, 24);
    let text = screen_text(&app);
    for needle in [
        "DIFFKIT",
        "CHANGES (2)",
        "const age = 20;",
        "const age = 21;",
        "OLD · index",
    ] {
        assert!(text.contains(needle), "{needle}:\n{text}");
    }
}

#[test]
fn narrow_terminal_falls_back_clearly() {
    let (_t, repo) = sample("render-narrow");
    let mut app = tui_app(&repo.path, 50, 20);
    let text = screen_text(&app);
    // The warning wraps inside the 28-column panel.
    assert!(
        text.contains("Too narrow for side-by-side") && text.contains("needs 35"),
        "{text}"
    );
    app.dispatch(Action::ToggleSidebar);
    let text = screen_text(&app);
    assert!(
        text.contains("const age = 21;"),
        "without the sidebar it fits:\n{text}"
    );
}

#[test]
fn too_small_terminals_show_a_warning() {
    let (_t, repo) = sample("render-small");
    for (w, h) in [(39, 30), (120, 9), (30, 5)] {
        let mut app = tui_app(&repo.path, 120, 40);
        app.dispatch(Action::Resize(w, h));
        let text = screen_text(&app);
        assert!(
            text.contains("Terminal too small") || h < 3,
            "{w}x{h}:\n{text}"
        );
    }
}

#[test]
fn rendering_never_panics_at_any_size() {
    let (_t, repo) = sample("render-sizes");
    repo.write(
        "untracked/a very long path name that keeps going and going/file.txt",
        "x\n",
    );
    let mut app = tui_app(&repo.path, 120, 40);
    for w in [1u16, 2, 10, 39, 40, 41, 50, 60, 80, 120, 250] {
        for h in [1u16, 2, 5, 9, 10, 11, 24, 40, 90] {
            app.dispatch(Action::Resize(w, h));
            render_buffer(&app);
            app.dispatch(Action::ToggleHelp);
            render_buffer(&app);
            app.dispatch(Action::CloseOverlay);
            app.dispatch(Action::ScrollBottom);
            app.dispatch(Action::ScrollRight);
            render_buffer(&app);
        }
    }
}

#[test]
fn large_sidebar_keeps_selection_visible() {
    let (_t, repo) = repo("render-many");
    for i in 0..150 {
        repo.write(format!("f{i:03}.txt"), "a\n");
    }
    repo.commit_all("base");
    for i in 0..150 {
        repo.write(format!("f{i:03}.txt"), "b\n");
    }
    let mut app = tui_app(&repo.path, 100, 30);
    app.dispatch(Action::SelectLastFile);
    let text = screen_text(&app);
    assert!(
        text.contains("M  f149.txt") && text.contains("file 150/150"),
        "{text}"
    );
    assert!(!text.contains("M  f000.txt"), "scrolled away from the top");
    app.dispatch(Action::SidebarPageUp);
    let text = screen_text(&app);
    assert!(
        text.contains(&format!("file {}/150", 150 - 24)),
        "page = visible rows:\n{text}"
    );
}

#[test]
fn long_file_names_are_truncated_from_the_start() {
    let (_t, repo) = repo("render-long-name");
    let dir = "a-really-long-directory-name/another-even-longer-directory-name";
    repo.write(format!("{dir}/final-file-name.rs"), "x\n");
    let app = tui_app(&repo.path, 100, 30);
    let lines = screen(&app);
    let row = lines
        .iter()
        .find(|l| l.contains("??") && l.contains("final-file-name.rs"))
        .unwrap();
    assert!(row.contains("…"), "{row}");
    assert!(row.contains("│?? …"), "status code stays visible: {row}");
}

#[test]
fn long_lines_scroll_horizontally_with_fixed_line_numbers() {
    let (_t, repo) = repo("render-long-line");
    repo.write("w.txt", "short\n");
    repo.commit_all("base");
    repo.write("w.txt", &format!("START{}END\n", "x".repeat(300)));
    let mut app = tui_app(&repo.path, 100, 20);
    let text = screen_text(&app);
    assert!(text.contains("1 + STARTxxx") && !text.contains("xEND"));
    for _ in 0..100 {
        app.dispatch(Action::ScrollRight);
    }
    let text = screen_text(&app);
    assert!(text.contains("xxxEND") && !text.contains("START"), "{text}");
    assert!(
        text.contains("1 + xxx"),
        "line number and marker stay put:\n{text}"
    );
}

#[test]
fn control_characters_never_reach_the_terminal() {
    let (_t, repo) = repo("render-escapes");
    std::fs::write(repo.path.join("evil.txt"), b"plain\n").unwrap();
    repo.commit_all("base");
    std::fs::write(repo.path.join("evil.txt"), b"red \x1b[31m alert\x07 \xff\n").unwrap();
    #[cfg(unix)]
    std::fs::write(repo.path.join("name\x1b[2Jwith-escape.txt"), b"x\n").unwrap();
    let app = tui_app(&repo.path, 120, 30);
    let buf = render_buffer(&app);
    let text = screen_text(&app);
    assert!(
        text.contains("red \u{241b}[31m alert\u{2407} \\xff"),
        "{text}"
    );
    for cell in buf.content() {
        assert!(
            !cell.symbol().chars().any(|c| c.is_control()),
            "control char in {:?}",
            cell.symbol()
        );
    }
    #[cfg(unix)]
    {
        let mut app = app;
        app.dispatch(Action::SetComparison(DiffComparison::Untracked));
        let text = screen_text(&app);
        assert!(text.contains("name\\u{1b}[2Jwith-escape.txt"), "{text}");
        assert!(!render_buffer(&app)
            .content()
            .iter()
            .any(|c| c.symbol().contains('\u{1b}')));
    }
}

#[test]
fn header_shows_tracking_and_detached_states() {
    let (t, repo) = sample("render-header");
    let bare = Repo::init_bare(t.join("remote.git"));
    repo.git(&[
        std::ffi::OsStr::new("remote"),
        "add".as_ref(),
        "origin".as_ref(),
        bare.path.as_os_str(),
    ]);
    repo.git(&["push", "-q", "-u", "origin", "main"]);
    repo.git(&["commit", "-q", "--allow-empty", "-m", "ahead"]);
    let app = tui_app(&repo.path, 140, 30);
    let header = &screen(&app)[0];
    assert!(
        header.contains("⎇ main ↑1 ↓0 vs origin/main (remote-tracking ref, as last fetched)"),
        "{header}"
    );
    repo.git(&["checkout", "-q", "--detach"]);
    let app = tui_app(&repo.path, 140, 30);
    let header = &screen(&app)[0];
    assert!(header.contains("HEAD detached at "), "{header}");
    assert!(!header.contains("⎇"), "no fabricated branch: {header}");
}
