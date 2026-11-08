//! Porcelain v2 status parsing from byte fixtures: every record type,
//! headers, unusual paths, and malformed input.

use diffkit::{git::status::*, types::repository::*};
use std::path::PathBuf;

const H: &str = "100644 100644 100644 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

fn one(s: &str) -> FileStatus {
    let mut files = parse(s.as_bytes()).unwrap().status.files;
    assert_eq!(files.len(), 1);
    files.remove(0)
}

#[test]
fn empty_output_is_clean() {
    let r = parse(b"").unwrap();
    assert!(r.status.is_clean());
    assert_eq!(r.headers, BranchHeaders::default());
}

#[test]
fn branch_headers() {
    let r = parse(
        b"# branch.oid 1111111111111111111111111111111111111111\0# branch.head feature/x y\0\
          # branch.upstream origin/feature/x\0# branch.ab +3 -2\0# stash 4\0",
    )
    .unwrap();
    assert_eq!(r.headers.head.as_deref(), Some(&b"feature/x y"[..]));
    assert_eq!(
        r.headers.upstream.as_deref(),
        Some(&b"origin/feature/x"[..])
    );
    assert_eq!(r.headers.ab.as_deref(), Some(&b"+3 -2"[..]));
    assert!(r.status.is_clean());
}

#[test]
fn ordinary_record_preserves_both_dimensions_and_spaces() {
    let f = one(&format!("1 MM N... {H} dir/a file  with spaces\0"));
    assert_eq!(f.path, PathBuf::from("dir/a file  with spaces"));
    assert_eq!((f.index, f.worktree), (Change::Modified, Change::Modified));
    assert_eq!(&f.xy, b"MM");
    assert_eq!(f.file_kind, Some(FileKind::Regular));
    assert!(f.is_partially_staged());
}

#[test]
fn deleted_file_kind_falls_back_to_head_mode() {
    let f = one("1 D. N... 120000 000000 000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0000000000000000000000000000000000000000 link\0");
    assert_eq!(f.index, Change::Deleted);
    assert_eq!(f.file_kind, Some(FileKind::Symlink));
}

#[test]
fn rename_record_consumes_original_path_field() {
    let r = parse(format!("2 R. N... {H} R87 new name\0old\nname\0? after\0").as_bytes()).unwrap();
    let f = &r.status.files[0];
    assert_eq!(f.path, PathBuf::from("new name"));
    assert_eq!(f.original_path, Some(PathBuf::from("old\nname")));
    assert_eq!(f.similarity, Some(Similarity(87)));
    assert_eq!(f.index, Change::Renamed);
    assert_eq!(r.status.files[1].path, PathBuf::from("after"));
    assert!(r.status.files[1].is_untracked());
}

#[test]
fn copy_record() {
    let r = parse(format!("2 C. N... {H} C100 copy\0src\0").as_bytes()).unwrap();
    let f = &r.status.files[0];
    assert!(f.is_copied() && f.is_staged() && !f.is_renamed());
    assert_eq!(f.original_path, Some(PathBuf::from("src")));
}

#[test]
fn unmerged_record_keeps_conflict_kind() {
    let f = one("u AU N... 000000 100644 000000 100644 0000000000000000000000000000000000000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0000000000000000000000000000000000000000 c d\0");
    assert_eq!(f.conflict, Some(ConflictKind::AddedByUs));
    assert_eq!(f.path, PathBuf::from("c d"));
    assert_eq!(&f.xy, b"AU");
    assert!(f.is_conflicted() && !f.is_staged() && !f.is_unstaged());
    let stages = f.stages.as_ref().unwrap();
    assert_eq!(
        (
            stages.base.is_some(),
            stages.ours.is_some(),
            stages.theirs.is_some()
        ),
        (false, true, false),
        "only our stage exists for AU"
    );
    assert_eq!(stages.kind(), f.conflict);
}

#[test]
fn unmerged_record_keeps_every_stage() {
    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";
    let f = one(&format!(
        "u UU N... 100644 100755 120000 100644 {A} {B} {C} f\0"
    ));
    let s = f.stages.unwrap();
    assert_eq!(s.base.as_ref().unwrap().oid.as_str(), A);
    assert_eq!(s.ours.as_ref().unwrap().mode, 0o100755);
    assert_eq!(s.theirs.as_ref().unwrap().kind(), Some(FileKind::Symlink));
    assert!(
        parse(format!("u UU N... 100644 100644 100644 100644 {A} zz {C} f\0").as_bytes()).is_err(),
        "a malformed stage object id is an error, not a missing stage"
    );
}

#[test]
fn untracked_and_ignored_are_kept_apart() {
    let r = parse(b"? new\tfile\0! target/out\0").unwrap();
    assert_eq!(r.status.files.len(), 1);
    assert!(r.status.files[0].is_untracked());
    assert_eq!(r.status.files[0].path, PathBuf::from("new\tfile"));
    assert_eq!(r.status.ignored, vec![PathBuf::from("target/out")]);
    assert_eq!(r.status.untracked_count(), 1);
}

#[test]
fn submodule_field() {
    let f = one("1 .M SCMU 160000 160000 160000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 sub\0");
    assert_eq!(f.file_kind, Some(FileKind::Submodule));
    assert_eq!(
        f.submodule,
        Some(SubmoduleState {
            commit_changed: true,
            has_tracked_changes: true,
            has_untracked_changes: true
        })
    );
}

#[test]
fn unknown_status_letter_is_preserved_not_rejected() {
    let f = one(&format!("1 .X N... {H} f\0"));
    assert_eq!(f.worktree, Change::Unknown(b'X'));
}

#[cfg(unix)]
#[test]
fn non_utf8_path_bytes_are_preserved() {
    use std::os::unix::ffi::OsStrExt;
    let mut raw = format!("1 .M N... {H} raw-").into_bytes();
    raw.extend_from_slice(b"\xff\xfe\0? \xc3\x28\0");
    let r = parse(&raw).unwrap();
    assert_eq!(
        r.status.files[0].path.as_os_str().as_bytes(),
        b"raw-\xff\xfe"
    );
    assert_eq!(r.status.files[1].path.as_os_str().as_bytes(), b"\xc3\x28");
}

#[test]
fn malformed_inputs_are_errors_not_panics() {
    let bad: &[&[u8]] = &[
        b"1 M. N...\0",
        b"1 M N... 100644 100644 100644 a b f\0",
        b"1 M. N... 10064x 100644 100644 a b f\0",
        b"2 R. N... 100644 100644 100644 a b R50 new\0",
        b"2 R. N... 100644 100644 100644 a b Q50 new\0old\0",
        b"u ZZ N... 100644 100644 100644 100644 a b c f\0",
        b"1 M. Nxyz 100644 100644 100644 a b f\0",
        b"? \0",
        b"Z what\0",
        b"\0",
        b"#\0",
        b"? unterminated",
    ];
    for input in bad {
        assert!(
            parse(input).is_err(),
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}
