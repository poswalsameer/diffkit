//! Filenames that break line- or space-based parsing, through every
//! record type, compared byte-for-byte with what was created.

use crate::support;

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use support::{diffkit, find, stdout, xy};
use support::{Repo, TempDir};

/// Names valid on Unix filesystems (APFS and ext4).
#[cfg(unix)]
const NAMES: &[&str] = &[
    "with space.txt",
    "  leading and trailing  ",
    "tab\there.txt",
    "new\nline.txt",
    "carriage\rreturn.txt",
    "quote\"double'single.txt",
    "back\\slash.txt",
    "#hash;semi&amp%20.txt",
    "-dash-leading.txt",
    "arrow -> name.txt",
    "glob*?[x].txt",
    "caf\u{e9}.txt",
    "\u{65e5}\u{672c}\u{8a9e}/\u{30d5}\u{30a1}\u{30a4}\u{30eb}.txt",
    "emoji \u{1f389}.txt",
    "dir with space/sub dir/file name.txt",
];

#[cfg(unix)]
#[test]
fn unusual_names_survive_every_record_type() {
    let tmp = TempDir::new("names-lifecycle");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("ordinary.txt", "keep ordinary names working");
    repo.commit_all("base");
    for name in NAMES {
        repo.write(name, name);
    }

    let s = repo.snapshot();
    for name in NAMES {
        assert!(find(&s, name).is_untracked(), "{name:?} untracked");
    }
    assert_eq!(s.status.files.len(), NAMES.len());

    repo.git(&["add", "-A"]);
    let s = repo.snapshot();
    for name in NAMES {
        assert_eq!(xy(find(&s, name)), "A.", "{name:?} staged");
    }

    repo.git(&["commit", "-q", "-m", "names"]);
    for name in NAMES {
        repo.write(name, "modified");
    }
    repo.write("ordinary.txt", "modified");
    let s = repo.snapshot();
    for name in NAMES {
        assert_eq!(xy(find(&s, name)), ".M", "{name:?} modified");
    }
    assert_eq!(xy(find(&s, "ordinary.txt")), ".M");
    repo.git(&["checkout", "-q", "--", "."]);

    // Renames carry a second NUL-separated path; both must round-trip.
    for (i, name) in NAMES.iter().enumerate() {
        let to = format!("renamed {i}\n\t{}", name.replace('/', "_"));
        repo.git(&["mv", "--", name, &to]);
    }
    let s = repo.snapshot();
    for (i, name) in NAMES.iter().enumerate() {
        let to = format!("renamed {i}\n\t{}", name.replace('/', "_"));
        let f = find(&s, &to);
        assert_eq!(xy(f), "R.", "{to:?}");
        assert_eq!(f.original_path.as_deref(), Some(Path::new(name)));
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_filename_from_real_git_index() {
    // APFS refuses to create non-UTF-8 names, but the index can hold them.
    // Adding an index entry produces genuine Git output with raw bytes on
    // every Unix filesystem.
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let tmp = TempDir::new("names-non-utf8-index");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("base", "b");
    repo.commit_all("base");
    let blob = {
        repo.write("content", "raw bytes");
        let oid = repo.git(&["hash-object", "-w", "content"]);
        repo.remove("content");
        oid.trim().to_owned()
    };
    let name: &[u8] = b"raw-\xff\xfe-\xc3\x28.txt";
    let mut cacheinfo = format!("100644,{blob},").into_bytes();
    cacheinfo.extend_from_slice(name);
    repo.git(&[
        OsStr::new("update-index"),
        OsStr::new("--add"),
        OsStr::new("--cacheinfo"),
        OsStr::from_bytes(&cacheinfo),
    ]);

    let s = repo.snapshot();
    let f = &s.status.files[0];
    assert_eq!(f.path.as_os_str().as_bytes(), name, "raw bytes preserved");
    assert_eq!(xy(f), "AD", "staged addition, absent from the working tree");

    repo.git(&["commit", "-q", "-m", "non-utf8"]);
    let s = repo.snapshot();
    let f = &s.status.files[0];
    assert_eq!(f.path.as_os_str().as_bytes(), name);
    assert_eq!(xy(f), ".D");

    let out = stdout(&diffkit(&repo.path, &["inspect"]));
    assert!(out.contains("\"raw-\\xff\\xfe-\\xc3(.txt\""), "{out}");
}

#[cfg(unix)]
#[test]
fn non_utf8_filename_on_disk_where_filesystem_allows() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let tmp = TempDir::new("names-non-utf8-disk");
    let repo = Repo::init(tmp.join("repo"));
    let name = OsStr::from_bytes(b"disk-\xff.txt");
    if let Err(e) = std::fs::write(repo.path.join(name), "x") {
        // Recorded in the test output; the index-based test above covers
        // the parser on such filesystems.
        eprintln!("SKIPPED on this filesystem: cannot create non-UTF-8 name: {e}");
        return;
    }
    let s = repo.snapshot();
    assert_eq!(s.status.files[0].path.as_os_str(), name);
    assert!(s.status.files[0].is_untracked());
}

#[cfg(unix)]
#[test]
fn unusual_names_are_escaped_in_cli_output() {
    let tmp = TempDir::new("names-cli");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("new\nline.txt", "x");
    repo.write("tab\there.txt", "x");
    repo.write("caf\u{e9} space.txt", "x");
    let out = stdout(&diffkit(&repo.path, &["inspect"]));
    assert!(
        out.contains("  ??  \"new\\nline.txt\"  (untracked)\n"),
        "{out}"
    );
    assert!(
        out.contains("  ??  \"tab\\there.txt\"  (untracked)\n"),
        "{out}"
    );
    assert!(
        out.contains("  ??  caf\u{e9} space.txt  (untracked)\n"),
        "{out}"
    );
}

#[test]
fn unicode_and_space_names_portable_subset() {
    let tmp = TempDir::new("names-portable");
    let repo = Repo::init(tmp.join("repo"));
    let names = [
        "a b c.txt",
        "caf\u{e9}.txt",
        "\u{4e2d}\u{6587}/\u{6587}\u{4ef6}.txt",
    ];
    for n in names {
        repo.write(n, n);
    }
    let s = repo.snapshot();
    let mut got: Vec<PathBuf> = s.status.files.iter().map(|f| f.path.clone()).collect();
    got.sort();
    let mut want: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
    want.sort();
    assert_eq!(got, want);
}
