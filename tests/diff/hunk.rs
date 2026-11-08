//! Hunks from parsed patches: independent line numbers per side, line
//! endings, range validation, and file content split into lines.

use diffkit::diff::hunk::*;
use diffkit::diff::parser::parse_patch;
use diffkit::types::diff::*;

fn hunks(patch: &str) -> Vec<DiffHunk> {
    let files = parse_patch(patch.as_bytes()).unwrap();
    build_hunks(files[0].hunks.clone(), &AlignmentOptions::default()).unwrap()
}

fn numbers(h: &DiffHunk) -> Vec<(DiffRowKind, Option<usize>, Option<usize>)> {
    h.rows
        .iter()
        .map(|r| {
            (
                r.kind,
                r.old_line.as_ref().map(|l| l.line_number),
                r.new_line.as_ref().map(|l| l.line_number),
            )
        })
        .collect()
}

use DiffRowKind::*;

#[test]
fn line_numbers_are_independent_per_side() {
    let h = &hunks("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -10,4 +20,4 @@\n a\n-let b = 1;\n+let b = 2;\n+new\n c\n-d\n")[0];
    assert_eq!(
        numbers(h),
        vec![
            (Context, Some(10), Some(20)),
            (Modified, Some(11), Some(21)),
            (Added, None, Some(22)),
            (Context, Some(12), Some(23)),
            (Removed, Some(13), None),
        ]
    );
}

#[test]
fn zero_count_hunks() {
    let h = &hunks("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,0 +4,2 @@\n+a\n+b\n")[0];
    assert_eq!(
        numbers(h),
        vec![(Added, None, Some(4)), (Added, None, Some(5))]
    );
    let h = &hunks("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -5,2 +4,0 @@\n-a\n-b\n")[0];
    assert_eq!(
        numbers(h),
        vec![(Removed, Some(5), None), (Removed, Some(6), None)]
    );
}

#[test]
fn line_endings_are_decoded() {
    let h = &hunks("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+b\n\\ No newline at end of file\n")[0];
    let ctx = h.rows[0].old_line.as_ref().unwrap();
    assert_eq!(
        (ctx.content.as_slice(), ctx.ending),
        (&b"a"[..], LineEnding::CrLf)
    );
    let m = &h.rows[1];
    assert_eq!(m.kind, Modified);
    assert_eq!(m.old_line.as_ref().unwrap().ending, LineEnding::CrLf);
    let new = m.new_line.as_ref().unwrap();
    assert_eq!(
        (new.content.as_slice(), new.ending, new.has_newline()),
        (&b"b"[..], LineEnding::None, false)
    );
}

#[test]
fn carriage_return_without_newline_stays_in_content() {
    let h = &hunks(
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1 @@\n+x\r\n\\ No newline at end of file\n",
    )[0];
    let l = h.rows[0].new_line.as_ref().unwrap();
    assert_eq!(
        (l.content.as_slice(), l.ending),
        (&b"x\r"[..], LineEnding::None)
    );
}

#[test]
fn overlapping_hunks_are_rejected() {
    let files = parse_patch(
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -5,2 +5,2 @@\n a\n b\n@@ -4,1 +4,1 @@\n c\n",
    )
    .unwrap();
    assert!(build_hunks(files[0].hunks.clone(), &AlignmentOptions::default()).is_err());
}

#[test]
fn split_lines_preserves_endings() {
    let l = split_lines(b"a\r\nb\n\nc");
    let got: Vec<_> = l
        .iter()
        .map(|l| (l.content.as_slice(), l.ending, l.line_number))
        .collect();
    assert_eq!(
        got,
        vec![
            (&b"a"[..], LineEnding::CrLf, 1),
            (b"b", LineEnding::Lf, 2),
            (b"", LineEnding::Lf, 3),
            (b"c", LineEnding::None, 4)
        ]
    );
    assert!(split_lines(b"").is_empty());
}

#[test]
fn addition_hunk_header_matches_git() {
    assert_eq!(
        addition_hunk(split_lines(b"one\n")).unwrap().header,
        b"@@ -0,0 +1 @@"
    );
    let h = addition_hunk(split_lines(b"a\nb\nc")).unwrap();
    assert_eq!(h.header, b"@@ -0,0 +1,3 @@");
    assert_eq!(h.new_count, 3);
    assert!(addition_hunk(Vec::new()).is_none());
}
