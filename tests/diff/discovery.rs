//! Raw/numstat discovery output: record pairing, stat-only entries,
//! malformed input, and file kinds.

use diffkit::{
    diff::discovery::*,
    types::diff::{DiffComparison, FileChangeKind, LineStats},
};
use std::path::Path;

const Z: &str = "0000000000000000000000000000000000000000";
const A: &str = "1111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222";

#[test]
fn parses_raw_and_numstat_pairs() {
    let out = format!(
        ":100644 100644 {A} {Z} M\0f\0:100644 100644 {A} {B} R097\0old\0new name\0:000000 100644 {Z} {B} A\0bin\0\
         1\t1\tf\0\x31\t0\t\0old\0new name\0-\t-\tbin\0"
    );
    let r = parse_raw_numstat(out.as_bytes()).unwrap().pairs;
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].0.new_oid, None, "zero id means not hashed");
    assert_eq!(r[1].0.score, Some(97));
    assert_eq!(r[1].0.path2.as_deref(), Some(&b"new name"[..]));
    assert_eq!(
        r[1].1.stats,
        Some(LineStats {
            added: 1,
            removed: 0
        })
    );
    assert_eq!(r[2].1.stats, None);
}

#[test]
fn paths_may_contain_tabs_newlines_and_colons() {
    let out = format!(":100644 100644 {A} {B} M\0:a\tb\nc\x001\t2\t:a\tb\nc\0");
    let r = parse_raw_numstat(out.as_bytes()).unwrap().pairs;
    assert_eq!(r[0].0.path, b":a\tb\nc");
    assert_eq!(
        r[0].1.stats,
        Some(LineStats {
            added: 1,
            removed: 2
        })
    );
}

#[test]
fn stat_only_records_are_dropped_and_counted() {
    let out = format!(
        ":100644 100644 {A} {Z} M\0touched\0:100644 100644 {A} {Z} M\0real\x001\t0\treal\0"
    );
    let d = parse_raw_numstat(out.as_bytes()).unwrap();
    assert_eq!(d.stat_only, 1);
    assert_eq!(d.pairs.len(), 1);
    assert_eq!(d.pairs[0].0.path, b"real");
}

#[test]
fn malformed_output_is_an_error() {
    for bad in [
        "1\t1\tf\0".to_string(),
        format!(":100644 {A} {B} M\0f\x001\t1\tf\0"),
        format!(":100644 100644 {A} {B} M\0f\x001\t1\tg\0"),
        format!(":100644 100644 {A} {B} M\0f\0x\ty\tf\0"),
        format!(":100644 100644 {A} {B} M\0f"),
    ] {
        assert!(parse_raw_numstat(bad.as_bytes()).is_err(), "{bad:?}");
    }
}

#[test]
fn unmerged_pair_is_folded_and_kinds_are_classified() {
    let out = format!(
        ":000000 100644 {Z} {A} U\0c\0:100644 100644 {A} {Z} M\0c\0:100644 100755 {A} {A} M\0x\0:100644 120000 {A} {B} T\0t\0\
         0\t0\tc\x004\t0\tc\x000\t0\tx\x001\t1\tt\0"
    );
    let recs = parse_raw_numstat(out.as_bytes()).unwrap().pairs;
    let files = build_files(
        recs,
        DiffComparison::Staged,
        Path::new("/nonexistent"),
        |_| false,
    )
    .unwrap();
    let kinds: Vec<_> = files.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [
            FileChangeKind::Unmerged,
            FileChangeKind::ModeChanged,
            FileChangeKind::TypeChanged
        ]
    );
    assert!(files[0].conflicted && files[0].stats.is_none());
    assert_eq!(files.iter().map(|f| f.id.0).collect::<Vec<_>>(), [0, 1, 2]);
}
