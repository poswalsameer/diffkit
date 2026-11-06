//! Symlinks are shown by their target text and never followed: broken
//! links, links leaving the repository, deletion, and replacement by a
//! regular file. Unix only (creating symlinks on Windows needs privileges
//! Diffkit's tests do not assume).
#![cfg(unix)]

mod common;

use common::{
    assert_read_only, discover, engine, entry, rows, screen_text, tui_app, Repo, TempDir,
};
use diffkit::{
    diff::{DiffComparison, DiffComparison::*, DiffContent, FileChangeKind},
    tui::Action,
};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

/// The panel text for `path` in comparison `c`.
fn panel(repo: &Repo, c: DiffComparison, path: &str) -> String {
    let mut app = tui_app(&repo.path, 150, 40);
    app.dispatch(Action::SetComparison(c));
    for _ in 0..app.mode_state(c).files().len() {
        if app.selected_file().unwrap().path() == std::path::Path::new(path) {
            return screen_text(&app);
        }
        app.dispatch(Action::SelectNextFile);
    }
    panic!("{path} not in {c:?}");
}

#[test]
fn target_change_is_a_text_change_of_the_target() {
    let (_t, repo) = repo("link-target");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.symlink("a.txt", "link");
    repo.commit_all("base");
    repo.symlink("b.txt", "link");
    let e = engine(&repo.path);
    let d = assert_read_only(&repo.path, || common::load(&e, Unstaged, "link"));
    assert_eq!(d.file.kind, FileChangeKind::Modified);
    assert_eq!(d.file.old_mode.unwrap().0, 0o120000);
    let r = rows(&d);
    assert_eq!(
        (r[0].3.as_str(), r[0].4.as_str()),
        ("a.txt", "b.txt"),
        "{r:?}"
    );
    let text = panel(&repo, Unstaged, "link");
    assert!(
        text.contains("symlink (target shown, not followed)"),
        "{text}"
    );
}

#[test]
fn broken_and_escaping_links_are_never_followed() {
    let (t, repo) = repo("link-broken");
    let secret = t.join("outside-secret.txt");
    std::fs::write(&secret, "TOP SECRET CONTENT\n").unwrap();
    repo.write("keep.txt", "k\n");
    repo.symlink("does/not/exist", "tracked-broken");
    repo.commit_all("base");
    repo.symlink("still/missing", "tracked-broken");
    repo.symlink("nowhere", "untracked-broken");
    repo.symlink(secret.to_str().unwrap(), "untracked-escape");
    let e = engine(&repo.path);
    assert_read_only(&repo.path, || {
        let d = common::load(&e, Unstaged, "tracked-broken");
        let r = rows(&d);
        let old: Vec<&str> = r
            .iter()
            .filter(|r| r.1.is_some())
            .map(|r| r.3.as_str())
            .collect();
        let new: Vec<&str> = r
            .iter()
            .filter(|r| r.2.is_some())
            .map(|r| r.4.as_str())
            .collect();
        assert_eq!(
            (old, new),
            (vec!["does/not/exist"], vec!["still/missing"]),
            "{r:?}"
        );
        let d = common::load(&e, Untracked, "untracked-broken");
        assert_eq!(rows(&d)[0].4, "nowhere");
        let d = common::load(&e, Untracked, "untracked-escape");
        let r = rows(&d);
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].4,
            secret.to_str().unwrap(),
            "the target path, not its content"
        );
        assert!(!format!("{d:?}").contains("TOP SECRET"));
    });
    let text = panel(&repo, Untracked, "untracked-escape");
    assert!(!text.contains("TOP SECRET"), "{text}");
}

#[test]
fn deletion_and_replacement_by_a_regular_file() {
    let (_t, repo) = repo("link-replace");
    repo.write("target.txt", "t\n");
    repo.symlink("target.txt", "to-delete");
    repo.symlink("target.txt", "to-replace");
    repo.commit_all("base");
    std::fs::remove_file(repo.path.join("to-delete")).unwrap();
    std::fs::remove_file(repo.path.join("to-replace")).unwrap();
    repo.write("to-replace", "now a regular file\n");
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    let deleted = entry(&list, "to-delete");
    assert_eq!(deleted.kind, FileChangeKind::Deleted);
    assert_eq!(deleted.old_mode.unwrap().0, 0o120000);
    let replaced = entry(&list, "to-replace");
    assert_eq!(replaced.kind, FileChangeKind::TypeChanged);
    let d = e.load_file_diff(&list, replaced.id).unwrap();
    let r = rows(&d);
    assert!(r.iter().any(|r| r.3 == "target.txt"), "{r:?}");
    assert!(r.iter().any(|r| r.4 == "now a regular file"), "{r:?}");
    let text = panel(&repo, Unstaged, "to-replace");
    assert!(
        text.contains("Type changed · symlink → regular file"),
        "{text}"
    );
}

#[test]
fn a_file_holding_a_path_is_not_a_symlink() {
    let (_t, repo) = repo("link-lookalike");
    repo.write("target.txt", "t\n");
    repo.symlink("target.txt", "real-link");
    repo.write_bytes("fake-link", b"target.txt");
    repo.git(&["add", "-A"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    let real = entry(&list, "real-link");
    let fake = entry(&list, "fake-link");
    assert_eq!(real.new_mode.unwrap().0, 0o120000);
    assert_eq!(fake.new_mode.unwrap().0, 0o100644);
    // Same text, different kinds; the interface says which is which.
    let real_text = panel(&repo, Staged, "real-link");
    let fake_text = panel(&repo, Staged, "fake-link");
    assert!(real_text.contains("symlink (target shown, not followed)"));
    assert!(!fake_text.contains("symlink"), "{fake_text}");
    for d in [
        e.load_file_diff(&list, real.id).unwrap(),
        e.load_file_diff(&list, fake.id).unwrap(),
    ] {
        assert!(matches!(d.content, DiffContent::Text { .. }));
        assert_eq!(rows(&d)[0].4, "target.txt");
    }
}
