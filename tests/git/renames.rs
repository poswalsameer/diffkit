//! Renames as Git reports them: across directories, with unusual names,
//! in every comparison and in commits, and never invented for an unrelated
//! deletion plus addition.

use crate::support;

use diffkit::{
    git::history,
    tui::actions::Action,
    types::diff::{DiffComparison::*, DiffContent, DiffFile, DiffFileList, FileChangeKind},
};
use std::path::Path;
use support::{
    assert_read_only, discover, engine, entry, numbered, screen_text, tui_app, Repo, TempDir,
};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

fn renamed<'a>(list: &'a DiffFileList, old: &str, new: &str) -> &'a DiffFile {
    let f = entry(list, new);
    assert_eq!(f.kind, FileChangeKind::Renamed, "{new}: {f:#?}");
    assert_eq!(f.old_path.as_deref(), Some(Path::new(old)));
    assert_eq!(f.new_path.as_deref(), Some(Path::new(new)));
    f
}

#[test]
fn rename_across_directories_with_and_without_edits() {
    let (_t, repo) = repo("rename-dirs");
    repo.write("src/deep/a/kept.rs", &numbered("kept ", 40));
    repo.write("src/deep/a/edited.rs", &numbered("edited ", 40));
    repo.commit_all("base");
    std::fs::create_dir_all(repo.path.join("lib/other")).unwrap();
    repo.git(&["mv", "src/deep/a/kept.rs", "lib/other/kept_moved.rs"]);
    std::fs::create_dir_all(repo.path.join("pkg")).unwrap();
    repo.git(&["mv", "src/deep/a/edited.rs", "pkg/edited.rs"]);
    repo.write(
        "pkg/edited.rs",
        &numbered("edited ", 40).replace("edited 7\n", "edited seven\n"),
    );
    repo.git(&["add", "pkg/edited.rs"]);
    let e = engine(&repo.path);
    assert_read_only(&repo.path, || {
        for c in [Staged, Combined] {
            let list = discover(&e, c);
            let pure = renamed(&list, "src/deep/a/kept.rs", "lib/other/kept_moved.rs");
            assert_eq!(pure.similarity, Some(100), "{c:?}");
            let d = e.load_file_diff(&list, pure.id).unwrap();
            assert!(d.hunks().is_empty(), "rename only: no hunks");
            let edited = renamed(&list, "src/deep/a/edited.rs", "pkg/edited.rs");
            assert!(edited.similarity.unwrap() < 100);
            let d = e.load_file_diff(&list, edited.id).unwrap();
            let rows = support::rows(&d);
            assert!(
                rows.iter()
                    .any(|r| r.3 == "edited 7" && r.4 == "edited seven"),
                "{rows:?}"
            );
        }
    });
}

#[test]
fn unrelated_deletion_and_addition_are_not_a_rename() {
    let (_t, repo) = repo("rename-not");
    repo.write("old.txt", &numbered("alpha ", 30));
    repo.commit_all("base");
    repo.remove("old.txt");
    repo.write("new.txt", &numbered("completely different ", 30));
    repo.git(&["add", "-A"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    assert_eq!(entry(&list, "old.txt").kind, FileChangeKind::Deleted);
    assert_eq!(entry(&list, "new.txt").kind, FileChangeKind::Added);
    assert!(list.files.iter().all(|f| f.kind != FileChangeKind::Renamed));
    // Unstaged: a deleted tracked file and a new untracked file are two
    // entries in two comparisons; Diffkit does not pair them.
    repo.git(&["reset", "-q"]);
    let unstaged = discover(&e, Unstaged);
    assert_eq!(entry(&unstaged, "old.txt").kind, FileChangeKind::Deleted);
    assert_eq!(
        discover(&e, Untracked).files[0].path(),
        Path::new("new.txt")
    );
}

#[test]
fn renames_with_unusual_names_keep_both_paths_exact() {
    let (_t, repo) = repo("rename-names");
    let pairs = [
        ("spaced name ü.txt", "dir with space/renamed é.txt"),
        ("-starts-with-dash", ":(glob)*not a pattern"),
        ("tab\there.txt", "new\nline.txt"),
    ];
    for (i, (old, _)) in pairs.iter().enumerate() {
        repo.write(old, &numbered(&format!("file{i} "), 20));
    }
    repo.commit_all("base");
    for (old, new) in pairs {
        if let Some(dir) = Path::new(new)
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
        {
            std::fs::create_dir_all(repo.path.join(dir)).unwrap();
        }
        repo.git(&["mv", "--", old, new]);
    }
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    for (old, new) in pairs {
        let f = renamed(&list, old, new);
        let d = e.load_file_diff(&list, f.id).unwrap();
        assert!(matches!(d.content, DiffContent::Text { .. }), "{new}");
    }
    // The interface shows both names, escaped.
    let mut app = tui_app(&repo.path, 160, 40);
    app.dispatch(Action::SetComparison(Staged));
    let mut labels = Vec::new();
    for _ in 0..list.len() {
        labels.push(screen_text(&app));
        app.dispatch(Action::SelectNextFile);
    }
    let all = labels.join("\n");
    assert!(
        all.contains("spaced name ü.txt → dir with space/renamed é.txt"),
        "{all}"
    );
    assert!(
        all.contains("\"tab\\there.txt\" → \"new\\nline.txt\""),
        "{all}"
    );
    assert!(
        all.contains("Renamed · was \"tab\\there.txt\" (100% similar)"),
        "{all}"
    );
}

#[test]
fn commit_diff_keeps_git_reported_renames() {
    let (_t, repo) = repo("rename-commit");
    repo.write("a/one.txt", &numbered("one ", 25));
    repo.write("gone.txt", &numbered("gone ", 25));
    repo.commit_all("base");
    std::fs::create_dir_all(repo.path.join("b")).unwrap();
    repo.git(&["mv", "a/one.txt", "b/one.txt"]);
    repo.remove("gone.txt");
    repo.write("fresh.txt", &numbered("fresh unrelated ", 25));
    let id = repo.commit_all("move");
    let e = engine(&repo.path);
    let s = repo.snapshot();
    let c = history::commit(
        &repo.path,
        &s.branch.head_commit.unwrap(),
        Path::new("/none"),
    )
    .unwrap();
    assert_eq!(c.id.as_str(), id);
    let list = e.discover_commit(&c.id, c.first_parent()).unwrap();
    renamed(&list, "a/one.txt", "b/one.txt");
    assert_eq!(entry(&list, "gone.txt").kind, FileChangeKind::Deleted);
    assert_eq!(entry(&list, "fresh.txt").kind, FileChangeKind::Added);
    assert_eq!(list.len(), 3);
}

/// macOS and Windows file systems are usually case-insensitive; a
/// case-only rename must still be reported exactly as Git records it.
#[test]
fn case_only_rename_is_reported_as_git_records_it() {
    let (t, repo) = repo("rename-case");
    let probe = t.join("CaseProbe");
    std::fs::write(&probe, "").unwrap();
    let insensitive = t.join("caseprobe").exists();
    repo.write("Readme.md", &numbered("readme ", 10));
    repo.commit_all("base");
    repo.git(&["mv", "Readme.md", "README.md"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    let f = renamed(&list, "Readme.md", "README.md");
    assert_eq!(f.similarity, Some(100));
    let d = e.load_file_diff(&list, f.id).unwrap();
    assert!(d.hunks().is_empty());
    // On a case-insensitive file system the working tree has one file,
    // reachable by either name; nothing is unstaged.
    if insensitive {
        assert!(discover(&e, Unstaged).is_empty());
    }
    eprintln!("file system case-insensitive: {insensitive}");
}
