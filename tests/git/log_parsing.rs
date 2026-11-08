//! `git log` record parsing: fields, parents, shallow boundaries, and
//! malformed output.

use diffkit::git::history::*;
use std::collections::HashSet;

const A: &str = "1111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222";
const C: &str = "3333333333333333333333333333333333333333";

fn record(id: &str, parents: &str, msg: &str) -> String {
    format!(
        "{id}\0{}\0{parents}\0Ann\0ann@example.invalid\02026-01-02T03:04:05+00:00\0Cid\0cid@example.invalid\02026-01-03T03:04:05+01:00\0{msg}\0",
        &id[..7]
    )
}

#[test]
fn parses_records_with_multiline_messages_and_merges() {
    let out = format!(
        "{}\n{}\n",
        record(A, &format!("{B} {C}"), "Merge topic\n\nBody line\n"),
        record(B, "", "root\n")
    );
    let shallow = HashSet::from([B.to_owned()]);
    let c = parse(out.as_bytes(), &shallow).unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].id.as_str(), A);
    assert_eq!(c[0].short_id, "1111111");
    assert!(c[0].is_merge());
    assert_eq!(c[0].first_parent().unwrap().as_str(), B);
    assert_eq!(c[0].subject(), "Merge topic");
    assert_eq!(c[0].message, "Merge topic\n\nBody line");
    assert_eq!(c[0].author.email, "ann@example.invalid");
    assert_eq!(c[0].committer.date, "2026-01-03T03:04:05+01:00");
    assert!(c[1].parents.is_empty() && c[1].shallow_boundary);
    assert!(!c[0].shallow_boundary);
}

#[test]
fn empty_output_is_no_commits() {
    assert!(parse(b"", &HashSet::new()).unwrap().is_empty());
}

#[test]
fn malformed_records_are_errors() {
    let good = record(A, "", "m");
    for bad in [
        good.replacen(A, "nothex", 1),
        good.replacen("\0m\0", "\0m", 1),
        format!("{good}junk"),
        format!("{good}{}", record(B, "zz", "x")),
        format!("{good}{}", record(B, "", "x")),
    ] {
        assert!(parse(bad.as_bytes(), &HashSet::new()).is_err(), "{bad:?}");
    }
}
