//! Git argument arrays for discovery and patches: the comparison each
//! selects, option placement, and literal paths after `--`.

use diffkit::{diff::commands::*, types::diff::DiffComparison, types::repository::ObjectId};
use std::{ffi::OsString, path::Path};

fn oid() -> ObjectId {
    ObjectId::parse(&[b'a'; 40]).unwrap()
}

fn head(base: &ObjectId) -> Range<'_> {
    Range { base, target: None }
}

#[test]
fn comparisons_select_distinct_sides() {
    let o = oid();
    let staged = discovery(DiffComparison::Staged, head(&o));
    let unstaged = discovery(DiffComparison::Unstaged, head(&o));
    let combined = discovery(DiffComparison::Combined, head(&o));
    let base = OsString::from("a".repeat(40));
    assert_eq!(staged[..2], ["diff-index", "--cached"].map(OsString::from));
    assert_eq!(staged.last().unwrap(), &base);
    assert_eq!(unstaged[0], "diff-files");
    assert!(!unstaged.contains(&base));
    assert_eq!(combined[0], "diff-index");
    assert!(!combined.contains(&OsString::from("--cached")));
    assert_eq!(combined.last().unwrap(), &base);
}

#[test]
fn patch_paths_follow_double_dash() {
    let o = oid();
    let a = patch(
        DiffComparison::Unstaged,
        head(&o),
        5,
        &[Path::new("-weird name"), Path::new(":(glob)*")],
    );
    let dd = a.iter().position(|x| x == "--").unwrap();
    assert_eq!(
        a[dd + 1..],
        [OsString::from("-weird name"), OsString::from(":(glob)*")]
    );
    assert!(a.contains(&OsString::from("-U5")));
    for opt in [
        "--no-ext-diff",
        "--no-textconv",
        "--full-index",
        "--diff-algorithm=histogram",
    ] {
        assert!(a.contains(&OsString::from(opt)), "{opt}");
    }
}

#[test]
fn commit_inspection_compares_parent_and_commit_trees() {
    let (p, c) = (oid(), ObjectId::parse(&[b'b'; 40]).unwrap());
    let range = Range {
        base: &p,
        target: Some(&c),
    };
    let d = discovery(DiffComparison::Commit, range);
    assert_eq!(d[..2], ["diff-tree", "-r"].map(OsString::from));
    assert_eq!(
        d[d.len() - 2..],
        [
            OsString::from("a".repeat(40)),
            OsString::from("b".repeat(40))
        ]
    );
    let pt = patch(DiffComparison::Commit, range, 3, &[Path::new("f")]);
    let dd = pt.iter().position(|x| x == "--").unwrap();
    assert_eq!(pt[dd - 1], OsString::from("b".repeat(40)));
    assert!(pt.contains(&OsString::from("--no-textconv")));
}
