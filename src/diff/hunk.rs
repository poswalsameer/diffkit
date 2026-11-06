//! Turns parsed hunks into [`DiffHunk`]s: validates ranges, assigns old
//! and new line numbers independently, decodes line endings, and aligns
//! each change block into side-by-side rows.

use super::{
    alignment,
    errors::{DiffError, Result},
    models::{
        AlignedRow, AlignmentOptions, AlignmentQuality, DiffHunk, DiffLine, DiffRowKind, LineEnding,
    },
    parser::{Origin, PatchLine, RawHunk},
};

pub fn build_hunks(raw: &[RawHunk], opts: &AlignmentOptions) -> Result<Vec<DiffHunk>> {
    let mut prev: Option<&RawHunk> = None;
    let mut out = Vec::with_capacity(raw.len());
    for h in raw {
        if let Some(p) = prev {
            if first(h.old_start, h.old_count) < end(p.old_start, p.old_count)
                || first(h.new_start, h.new_count) < end(p.new_start, p.new_count)
            {
                return Err(DiffError::parse(
                    h.patch_line,
                    "hunks overlap or are out of order",
                ));
            }
        }
        out.push(build_hunk(h, opts));
        prev = Some(h);
    }
    Ok(out)
}

/// First line a hunk covers. A zero-count range "starts" after its line.
fn first(start: usize, count: usize) -> usize {
    if count == 0 {
        start + 1
    } else {
        start
    }
}

/// One past the last line a hunk covers.
fn end(start: usize, count: usize) -> usize {
    if count == 0 {
        start + 1
    } else {
        start + count
    }
}

fn build_hunk(h: &RawHunk, opts: &AlignmentOptions) -> DiffHunk {
    let mut old_no = h.old_start;
    let mut new_no = h.new_start;
    let mut rows = Vec::with_capacity(h.lines.len());
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let mut quality = AlignmentQuality::Exact;
    let mut flush =
        |removed: &mut Vec<DiffLine>, added: &mut Vec<DiffLine>, rows: &mut Vec<AlignedRow>| {
            if removed.is_empty() && added.is_empty() {
                return;
            }
            let (block, q) = alignment::align(std::mem::take(removed), std::mem::take(added), opts);
            if q == AlignmentQuality::Bounded {
                quality = q;
            }
            rows.extend(block);
        };
    for line in &h.lines {
        match line.origin {
            Origin::Context => {
                flush(&mut removed, &mut added, &mut rows);
                rows.push(AlignedRow {
                    kind: DiffRowKind::Context,
                    old_line: Some(diff_line(line, old_no)),
                    new_line: Some(diff_line(line, new_no)),
                });
                old_no += 1;
                new_no += 1;
            }
            Origin::Removed => {
                removed.push(diff_line(line, old_no));
                old_no += 1;
            }
            Origin::Added => {
                added.push(diff_line(line, new_no));
                new_no += 1;
            }
        }
    }
    flush(&mut removed, &mut added, &mut rows);
    DiffHunk {
        old_start: h.old_start,
        old_count: h.old_count,
        new_start: h.new_start,
        new_count: h.new_count,
        header: h.header.clone(),
        section_heading: h.section_heading.clone(),
        rows,
        alignment: quality,
    }
}

fn diff_line(line: &PatchLine, line_number: usize) -> DiffLine {
    let (content, ending) = if line.no_newline {
        (line.text.clone(), LineEnding::None)
    } else if let Some(c) = line.text.strip_suffix(b"\r") {
        (c.to_vec(), LineEnding::CrLf)
    } else {
        (line.text.clone(), LineEnding::Lf)
    };
    DiffLine {
        content,
        line_number,
        ending,
    }
}

/// Splits file content into numbered lines, preserving LF, CRLF, and a
/// missing final newline.
pub fn split_lines(bytes: &[u8]) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let line_number = out.len() + 1;
        match rest.iter().position(|&b| b == b'\n') {
            Some(i) => {
                let line = &rest[..i];
                let (content, ending) = match line.strip_suffix(b"\r") {
                    Some(c) => (c, LineEnding::CrLf),
                    None => (line, LineEnding::Lf),
                };
                out.push(DiffLine {
                    content: content.to_vec(),
                    line_number,
                    ending,
                });
                rest = &rest[i + 1..];
            }
            None => {
                out.push(DiffLine {
                    content: rest.to_vec(),
                    line_number,
                    ending: LineEnding::None,
                });
                rest = &[];
            }
        }
    }
    out
}

/// A single hunk adding every line of a new file, in Git's header format.
pub fn addition_hunk(lines: Vec<DiffLine>) -> Option<DiffHunk> {
    if lines.is_empty() {
        return None;
    }
    let n = lines.len();
    let header = if n == 1 {
        b"@@ -0,0 +1 @@".to_vec()
    } else {
        format!("@@ -0,0 +1,{n} @@").into_bytes()
    };
    Some(DiffHunk {
        old_start: 0,
        old_count: 0,
        new_start: 1,
        new_count: n,
        header,
        section_heading: Vec::new(),
        rows: lines
            .into_iter()
            .map(|l| AlignedRow {
                kind: DiffRowKind::Added,
                old_line: None,
                new_line: Some(l),
            })
            .collect(),
        alignment: AlignmentQuality::Exact,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parser::parse_patch;

    fn hunks(patch: &str) -> Vec<DiffHunk> {
        let files = parse_patch(patch.as_bytes()).unwrap();
        build_hunks(&files[0].hunks, &AlignmentOptions::default()).unwrap()
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
        let h = &hunks("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1 @@\n+x\r\n\\ No newline at end of file\n")[0];
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
        assert!(build_hunks(&files[0].hunks, &AlignmentOptions::default()).is_err());
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
}
