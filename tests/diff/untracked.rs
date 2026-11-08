//! Untracked files: discovered by Git, read directly, never staged.

use crate::support;

use diffkit::{
    diff::errors::{DiffError, StaleReason},
    types::diff::{
        BinaryStatus, BinarySummary, DiffComparison::Untracked, DiffContent, DiffOptions,
        DiffRowKind::*, FileChangeKind, LineEnding,
    },
};
use std::path::{Path, PathBuf};
use support::{discover, engine, engine_with, entry, row, rows, Repo, TempDir};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    repo.write("tracked.txt", "t\n");
    repo.commit_all("base");
    (tmp, repo)
}

#[test]
fn untracked_contents_as_additions() {
    let (_t, repo) = repo("untracked-kinds");
    repo.write("plain.txt", "one\ntwo\n");
    repo.write("empty.txt", "");
    repo.write("no-eol.txt", "last");
    std::fs::write(repo.path.join("crlf.txt"), b"a\r\nb\r\n").unwrap();
    std::fs::write(repo.path.join("data.bin"), b"PNG\x00\x01\x02").unwrap();
    repo.write(
        "nested/deeper/\u{65e5}\u{672c} file.txt",
        "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\n",
    );
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    assert_eq!(list.len(), 6);
    for f in &list.files {
        assert_eq!(f.kind, FileChangeKind::Added);
        assert_eq!(
            (f.old_path.as_ref(), f.binary),
            (None, BinaryStatus::Unknown)
        );
    }
    let load = |p: &str| e.load_file_diff(&list, entry(&list, p).id).unwrap();

    let plain = load("plain.txt");
    assert_eq!(
        rows(&plain),
        vec![
            row(Added, None, Some(1), "", "one"),
            row(Added, None, Some(2), "", "two")
        ]
    );
    assert_eq!(plain.hunks()[0].header, b"@@ -0,0 +1,2 @@");
    assert_eq!(plain.file.binary, BinaryStatus::Text);
    assert_eq!(plain.file.new_mode.unwrap().0, 0o100644);

    assert_eq!(
        load("empty.txt").content,
        DiffContent::Text { hunks: vec![] }
    );
    let no_eol = load("no-eol.txt");
    assert_eq!(
        no_eol
            .rows()
            .next()
            .unwrap()
            .new_line
            .as_ref()
            .unwrap()
            .ending,
        LineEnding::None
    );
    let crlf = load("crlf.txt");
    assert!(crlf
        .rows()
        .all(|r| r.new_line.as_ref().unwrap().ending == LineEnding::CrLf));
    let bin = load("data.bin");
    assert_eq!(
        bin.content,
        DiffContent::Binary(BinarySummary {
            old_size: None,
            new_size: Some(6)
        })
    );
    assert_eq!(bin.file.binary, BinaryStatus::Binary);
    let uni = load("nested/deeper/\u{65e5}\u{672c} file.txt");
    assert_eq!(rows(&uni)[0].4, "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}");
}

#[test]
fn ignored_files_are_excluded() {
    let (_t, repo) = repo("untracked-ignored");
    repo.write(".gitignore", "*.log\nbuild/\n");
    repo.commit_all("ignore");
    repo.write("x.log", "ignored");
    repo.write("build/out.o", "ignored");
    std::fs::create_dir_all(repo.path.join(".git/info")).unwrap();
    std::fs::write(repo.path.join(".git/info/exclude"), "secret.txt\n").unwrap();
    repo.write("secret.txt", "excluded locally");
    repo.write("kept.txt", "kept");
    let list = discover(&engine(&repo.path), Untracked);
    let paths: Vec<PathBuf> = list.files.iter().map(|f| f.path().to_path_buf()).collect();
    assert_eq!(paths, vec![PathBuf::from("kept.txt")]);
}

#[test]
fn reading_untracked_files_does_not_touch_the_repository() {
    let (_t, repo) = repo("untracked-readonly");
    repo.write("new.txt", "content\n");
    let index = std::fs::read(repo.path.join(".git/index")).unwrap();
    let objects = repo.git(&["count-objects", "-v"]);
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    e.load_all(&list).unwrap();
    assert_eq!(std::fs::read(repo.path.join(".git/index")).unwrap(), index);
    assert_eq!(
        repo.git(&["count-objects", "-v"]),
        objects,
        "no blob was written"
    );
    assert!(
        repo.git(&["status", "--porcelain"]).contains("?? new.txt"),
        "still untracked"
    );
}

#[test]
fn size_limit_is_explicit() {
    let (_t, repo) = repo("untracked-limit");
    repo.write("big.txt", &"x".repeat(2000));
    repo.write("small.txt", "ok\n");
    let e = engine_with(
        &repo.path,
        DiffOptions {
            max_untracked_bytes: 1000,
            ..DiffOptions::default()
        },
    );
    let list = discover(&e, Untracked);
    let big = e.load_file_diff(&list, entry(&list, "big.txt").id).unwrap();
    assert_eq!(
        big.content,
        DiffContent::TooLarge {
            limit: 1000,
            size: Some(2000)
        }
    );
    let small = e
        .load_file_diff(&list, entry(&list, "small.txt").id)
        .unwrap();
    assert!(matches!(small.content, DiffContent::Text { .. }));
}

#[test]
fn vanished_and_changed_files_are_reported() {
    let (_t, repo) = repo("untracked-races");
    repo.write("gone.txt", "soon gone\n");
    repo.write("changing.txt", "v1\n");
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    std::fs::remove_file(repo.path.join("gone.txt")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    repo.write("changing.txt", "version two\n");
    match e.load_file_diff(&list, entry(&list, "gone.txt").id) {
        Err(DiffError::Vanished { path }) => assert_eq!(path, Path::new("gone.txt")),
        other => panic!("{other:?}"),
    }
    match e.load_file_diff(&list, entry(&list, "changing.txt").id) {
        Err(DiffError::Stale { reason, .. }) => assert_eq!(reason, StaleReason::WorkingTreeChanged),
        other => panic!("{other:?}"),
    }
    let fresh = discover(&e, Untracked);
    let d = e
        .load_file_diff(&fresh, entry(&fresh, "changing.txt").id)
        .unwrap();
    assert_eq!(rows(&d)[0].4, "version two");
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let (t, repo) = repo("untracked-symlinks");
    std::fs::write(t.join("outside-secret"), "do not read").unwrap();
    std::os::unix::fs::symlink(t.join("outside-secret"), repo.path.join("link-out")).unwrap();
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    let f = entry(&list, "link-out");
    assert_eq!(f.new_mode.unwrap().0, 0o120000);
    let d = e.load_file_diff(&list, f.id).unwrap();
    let r = rows(&d);
    assert_eq!(r.len(), 1);
    assert_eq!(
        r[0].4,
        t.join("outside-secret").to_str().unwrap(),
        "the target path, not its content"
    );
    assert!(!r[0].4.contains("do not read"));
}

#[cfg(unix)]
#[test]
fn directory_swapped_for_outside_symlink_is_refused() {
    let (t, repo) = repo("untracked-escape");
    repo.write("dir/file.txt", "inside\n");
    std::fs::create_dir_all(t.join("outside")).unwrap();
    std::fs::write(t.join("outside/file.txt"), "outside\n").unwrap();
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    std::fs::remove_dir_all(repo.path.join("dir")).unwrap();
    std::os::unix::fs::symlink(t.join("outside"), repo.path.join("dir")).unwrap();
    match e.load_file_diff(&list, entry(&list, "dir/file.txt").id) {
        Err(DiffError::OutsideRepository { path }) => assert_eq!(path, Path::new("dir/file.txt")),
        other => panic!("{other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_distinguished() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, repo) = repo("untracked-unreadable");
    repo.write("locked.txt", "secret\n");
    let p = repo.path.join("locked.txt");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    let d = e.load_file_diff(&list, entry(&list, "locked.txt").id);
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    if std::fs::read(&p).is_ok() && unsafe_root() {
        eprintln!("SKIPPED: running as root, permissions are not enforced");
        return;
    }
    assert!(matches!(d.unwrap().content, DiffContent::Unreadable { .. }));
}

#[cfg(unix)]
fn unsafe_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[test]
fn nested_repository_is_reported() {
    let (_t, repo) = repo("untracked-nested-repo");
    let inner = Repo::init(repo.path.join("inner"));
    inner.write("x", "x\n");
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    let f = entry(&list, "inner");
    let d = e.load_file_diff(&list, f.id).unwrap();
    assert_eq!(d.content, DiffContent::NestedRepository);
}

#[cfg(unix)]
#[test]
fn non_utf8_untracked_name_where_filesystem_allows() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let (_t, repo) = repo("untracked-non-utf8");
    let name = OsStr::from_bytes(b"raw-\xff.txt");
    if let Err(e) = std::fs::write(repo.path.join(name), "bytes\n") {
        eprintln!("SKIPPED on this filesystem: cannot create non-UTF-8 name: {e}");
        return;
    }
    let e = engine(&repo.path);
    let list = discover(&e, Untracked);
    let f = entry(&list, name);
    assert_eq!(rows(&e.load_file_diff(&list, f.id).unwrap())[0].4, "bytes");
}
