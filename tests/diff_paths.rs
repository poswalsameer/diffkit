//! Unusual and non-UTF-8 paths through discovery and per-file loading.

mod common;

use common::{discover, engine, entry, row, rows, Repo, TempDir};
#[cfg(unix)]
use diffkit::diff::FileChangeKind;
use diffkit::diff::{DiffComparison::*, DiffRowKind::*};
#[cfg(unix)]
use std::path::Path;

/// Names that break naive parsing or pathspec handling (valid on APFS and
/// ext4).
#[cfg(unix)]
const NAMES: &[&str] = &[
    "with space.txt",
    "  leading and trailing  ",
    "tab\there.txt",
    "new\nline.txt",
    "quote\"double'single.txt",
    "back\\slash.txt",
    "-dash-leading.txt",
    "arrow -> name.txt",
    "glob*?[ab].txt",
    ":(attr:nope)magic.txt",
    ":!excluded.txt",
    "caf\u{e9}/\u{65e5}\u{672c}\u{8a9e}.txt",
    "emoji \u{1f389}.txt",
];

#[cfg(unix)]
#[test]
fn unusual_names_in_every_comparison() {
    let tmp = TempDir::new("paths-unusual");
    let repo = Repo::init(tmp.join("repo"));
    for n in NAMES {
        repo.write(n, "original line\n");
    }
    repo.commit_all("base");
    for n in NAMES {
        repo.write(n, "original line, staged edit\n");
    }
    repo.git(&["add", "-A"]);
    for n in NAMES {
        repo.write(n, "original line, staged edit, then unstaged edit\n");
    }
    let e = engine(&repo.path);
    for c in [Staged, Unstaged, Combined] {
        let list = discover(&e, c);
        assert_eq!(list.len(), NAMES.len(), "{c:?}");
        for n in NAMES {
            let f = entry(&list, n);
            let d = e
                .load_file_diff(&list, f.id)
                .unwrap_or_else(|err| panic!("{c:?} {n:?}: {err}"));
            assert_eq!(d.file.path(), Path::new(n));
            assert_eq!(rows(&d).len(), 1, "{c:?} {n:?}");
            assert_eq!(rows(&d)[0].0, Modified);
        }
        let all = e.load_all(&list).unwrap();
        assert!(all.unexpected_paths.is_empty());
        assert!(all.files.iter().all(|l| l.result.is_ok()), "{c:?}");
    }
}

#[cfg(unix)]
#[test]
fn renames_keep_both_unusual_paths() {
    let tmp = TempDir::new("paths-rename");
    let repo = Repo::init(tmp.join("repo"));
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    for n in NAMES {
        repo.write(n, &format!("{n}\n{body}"));
    }
    repo.commit_all("base");
    std::fs::create_dir_all(repo.path.join("moved")).unwrap();
    for (i, n) in NAMES.iter().enumerate() {
        repo.git(&[
            "mv",
            "--",
            n,
            &format!("moved/{i} \t{}", n.replace('/', "_")),
        ]);
    }
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    for (i, n) in NAMES.iter().enumerate() {
        let to = format!("moved/{i} \t{}", n.replace('/', "_"));
        let f = entry(&list, &to);
        assert_eq!(f.kind, FileChangeKind::Renamed);
        assert_eq!(f.old_path.as_deref(), Some(Path::new(n)));
        let d = e
            .load_file_diff(&list, f.id)
            .unwrap_or_else(|err| panic!("{n:?}: {err}"));
        assert!(d.hunks().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_tracked_path() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let tmp = TempDir::new("paths-non-utf8");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("base", "b\n");
    repo.commit_all("base");
    repo.write("content", "raw name content\n");
    let blob = repo
        .git(&["hash-object", "-w", "content"])
        .trim()
        .to_owned();
    repo.remove("content");
    let name: &[u8] = b"raw-\xff\xfe.txt";
    let mut cacheinfo = format!("100644,{blob},").into_bytes();
    cacheinfo.extend_from_slice(name);
    repo.git(&[
        OsStr::new("update-index"),
        OsStr::new("--add"),
        OsStr::new("--cacheinfo"),
        OsStr::from_bytes(&cacheinfo),
    ]);
    let e = engine(&repo.path);
    let path = Path::new(OsStr::from_bytes(name));

    let staged = discover(&e, Staged);
    let f = entry(&staged, path);
    assert_eq!(f.kind, FileChangeKind::Added);
    let d = e.load_file_diff(&staged, f.id).unwrap();
    assert_eq!(
        rows(&d),
        vec![row(Added, None, Some(1), "", "raw name content")]
    );

    let unstaged = discover(&e, Unstaged);
    let f = entry(&unstaged, path);
    assert_eq!(
        f.kind,
        FileChangeKind::Deleted,
        "absent from the working tree"
    );
    let d = e.load_file_diff(&unstaged, f.id).unwrap();
    assert_eq!(
        rows(&d),
        vec![row(Removed, Some(1), None, "raw name content", "")]
    );
}

#[test]
fn portable_unicode_and_space_names() {
    let tmp = TempDir::new("paths-portable");
    let repo = Repo::init(tmp.join("repo"));
    let names = [
        "a b c.txt",
        "caf\u{e9}.txt",
        "\u{4e2d}\u{6587}/\u{6587}\u{4ef6}.txt",
    ];
    for n in names {
        repo.write(n, "1\n");
    }
    repo.commit_all("base");
    for n in names {
        repo.write(n, "1\n2\n");
    }
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    for n in names {
        let d = e.load_file_diff(&list, entry(&list, n).id).unwrap();
        assert_eq!(rows(&d)[1], row(Added, None, Some(2), "", "2"));
    }
}
