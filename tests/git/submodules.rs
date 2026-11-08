//! Submodules and nested repositories: gitlinks are distinguished from
//! files, their state is explained, and nothing inside them is listed,
//! initialized, or modified.

use crate::support;

use diffkit::{
    tui::actions::Action,
    types::diff::{DiffComparison::*, DiffContent, FileChangeKind, SubmoduleWorktree},
};
use std::{ffi::OsStr, path::Path};
use support::{assert_read_only, discover, engine, entry, screen_text, tui_app, Repo, TempDir};

/// Git refuses local `file` transport for submodules unless allowed.
const ALLOW_FILE: [&str; 2] = ["-c", "protocol.file.allow=always"];

/// A superproject `sup` with submodule `sub` (two commits) checked out.
fn superproject(label: &str) -> (TempDir, Repo, Repo) {
    let tmp = TempDir::new(label);
    let sub = Repo::init(tmp.join("sub-origin"));
    sub.write("inner.txt", "one\n");
    sub.commit_all("s1");
    sub.write("inner.txt", "two\n");
    sub.commit_all("s2");
    let sup = Repo::init(tmp.join("sup"));
    sup.write("top.txt", "top\n");
    sup.commit_all("top");
    let mut args: Vec<&OsStr> = ALLOW_FILE.iter().map(OsStr::new).collect();
    args.extend(["submodule", "add", "-q"].map(OsStr::new));
    args.push(sub.path.as_os_str());
    args.push(OsStr::new("sub"));
    sup.git(&args);
    sup.commit_all("add submodule");
    (tmp, sup, sub)
}

fn submodule_content(
    repo: &Repo,
    c: diffkit::types::diff::DiffComparison,
) -> diffkit::types::diff::SubmoduleChange {
    let e = engine(&repo.path);
    let list = discover(&e, c);
    let f = entry(&list, "sub");
    assert!(
        [f.old_mode, f.new_mode]
            .iter()
            .flatten()
            .any(|m| m.0 == 0o160000),
        "a gitlink, not a file: {f:#?}"
    );
    match e.load_file_diff(&list, f.id).unwrap().content {
        DiffContent::Submodule(s) => s,
        other => panic!("expected a submodule summary, got {other:?}"),
    }
}

fn inner_paths_listed(repo: &Repo) -> bool {
    let e = engine(&repo.path);
    [Staged, Unstaged, Combined, Untracked].iter().any(|&c| {
        discover(&e, c)
            .files
            .iter()
            .any(|f| f.path().components().count() > 1 && f.path().starts_with("sub"))
    })
}

#[test]
fn moved_submodule_commit_is_a_submodule_change_not_a_file() {
    let (_t, sup, _sub) = superproject("sub-moved");
    let inner = Repo {
        path: sup.path.join("sub"),
    };
    inner.write("inner.txt", "three\n");
    inner.git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t.invalid",
        "commit",
        "-qam",
        "s3",
    ]);
    let s = assert_read_only(&sup.path, || submodule_content(&sup, Unstaged));
    assert!(s.old_commit.is_some());
    assert_eq!(s.worktree, Some(SubmoduleWorktree::Populated));
    assert!(
        !inner_paths_listed(&sup),
        "submodule contents are not inspected"
    );
    let mut app = tui_app(&sup.path, 140, 40);
    app.dispatch(Action::SetComparison(Unstaged));
    let text = screen_text(&app);
    assert!(text.contains("Submodule (gitlink)"), "{text}");
    assert!(
        text.contains("Git reports: its checked-out commit differs from the recorded one."),
        "{text}"
    );
    assert!(
        text.contains("Submodule contents are not inspected"),
        "{text}"
    );
}

#[test]
fn uninitialized_submodule_is_explained() {
    let (t, sup, sub) = superproject("sub-uninit");
    // A clone without --recurse-submodules leaves `sub` as an empty directory.
    let clone = Repo::clone_from(&sup.path, t.join("clone"));
    assert!(clone.path.join("sub").is_dir());
    assert!(!clone.path.join("sub/.git").exists());
    // Record a different submodule commit in the index (test setup only).
    let older = sub.rev("HEAD~1");
    clone.git(&[
        "update-index",
        "--cacheinfo",
        &format!("160000,{older},sub"),
    ]);
    let s = assert_read_only(&clone.path, || submodule_content(&clone, Staged));
    assert_eq!(s.new_commit.as_ref().map(|c| c.to_string()), Some(older));
    assert_eq!(s.worktree, Some(SubmoduleWorktree::NotInitialized));
    let mut app = tui_app(&clone.path, 140, 40);
    app.dispatch(Action::SetComparison(Staged));
    let text = screen_text(&app);
    assert!(text.contains("Not initialized"), "{text}");
    assert!(
        !clone.path.join("sub/.git").exists(),
        "Diffkit never initializes a submodule"
    );
}

#[test]
fn missing_submodule_directory_is_explained() {
    let (_t, sup, _sub) = superproject("sub-missing");
    std::fs::remove_dir_all(sup.path.join("sub")).unwrap();
    let e = engine(&sup.path);
    let list = discover(&e, Unstaged);
    assert_eq!(entry(&list, "sub").kind, FileChangeKind::Deleted);
    let s = submodule_content(&sup, Unstaged);
    assert!(s.old_commit.is_some() && s.new_commit.is_none());
    assert_eq!(s.worktree, Some(SubmoduleWorktree::Missing));
    let mut app = tui_app(&sup.path, 140, 40);
    app.dispatch(Action::SetComparison(Unstaged));
    let text = screen_text(&app);
    assert!(
        text.contains("Its directory is missing from the working tree."),
        "{text}"
    );
}

#[test]
fn embedded_repository_added_as_gitlink() {
    let tmp = TempDir::new("sub-embedded");
    let outer = Repo::init(tmp.join("outer"));
    outer.write("top.txt", "top\n");
    outer.commit_all("top");
    let nested = Repo::init(outer.path.join("sub"));
    nested.write("inner.txt", "inner\n");
    nested.commit_all("inner");
    // `git add` of a repository records a gitlink without .gitmodules.
    outer.git(&["add", "sub"]);
    assert!(!outer.path.join(".gitmodules").exists());
    let s = assert_read_only(&outer.path, || submodule_content(&outer, Staged));
    assert!(s.old_commit.is_none());
    assert_eq!(
        s.new_commit.as_ref().map(|c| c.to_string()),
        Some(nested.head())
    );
    assert_eq!(s.worktree, Some(SubmoduleWorktree::Populated));
    assert!(!inner_paths_listed(&outer));
}

#[test]
fn untracked_nested_repository_is_not_listed_as_files() {
    let tmp = TempDir::new("sub-nested");
    let outer = Repo::init(tmp.join("outer"));
    outer.write("top.txt", "top\n");
    outer.commit_all("top");
    let nested = Repo::init(outer.path.join("vendor/nested"));
    nested.write("a.txt", "a\n");
    nested.write("deep/b.txt", "b\n");
    nested.commit_all("nested");
    nested.write("a.txt", "a edited\n");
    let e = engine(&outer.path);
    let untracked = assert_read_only(&outer.path, || discover(&e, Untracked));
    let paths: Vec<_> = untracked
        .files
        .iter()
        .map(|f| f.path().to_owned())
        .collect();
    assert_eq!(paths, vec![Path::new("vendor/nested").to_path_buf()]);
    let d = e.load_file_diff(&untracked, untracked.files[0].id).unwrap();
    assert_eq!(d.content, DiffContent::NestedRepository);
    for c in [Staged, Unstaged, Combined] {
        assert!(discover(&e, c).is_empty(), "{c:?}");
    }
    let mut app = tui_app(&outer.path, 140, 40);
    app.dispatch(Action::SetComparison(Untracked));
    let text = screen_text(&app);
    assert!(text.contains("Nested Git repository"), "{text}");
    assert!(text.contains("open Diffkit inside it"), "{text}");
}
