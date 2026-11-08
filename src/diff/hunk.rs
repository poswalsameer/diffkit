//! Turns parsed hunks into [`DiffHunk`]s: validates ranges, assigns old
//! and new line numbers independently, decodes line endings, and aligns
//! each change block into side-by-side rows.

use crate::{
    diff::{
        alignment,
        errors::{DiffError, Result},
        parser::{Origin, PatchLine, RawHunk},
    },
    types::diff::{
        AlignedRow, AlignmentOptions, AlignmentQuality, DiffHunk, DiffLine, DiffRowKind, LineEnding,
    },
};

/// Consumes the parsed hunks: line text moves into the result instead of
/// being copied (a context line is copied once, for its second side).
pub fn build_hunks(raw: Vec<RawHunk>, opts: &AlignmentOptions) -> Result<Vec<DiffHunk>> {
    // (old end, new end) of the previous hunk.
    let mut prev: Option<(usize, usize)> = None;
    let mut out = Vec::with_capacity(raw.len());
    for h in raw {
        if let Some((old_end, new_end)) = prev {
            if first(h.old_start, h.old_count) < old_end
                || first(h.new_start, h.new_count) < new_end
            {
                return Err(DiffError::parse(
                    h.patch_line,
                    "hunks overlap or are out of order",
                ));
            }
        }
        prev = Some((end(h.old_start, h.old_count), end(h.new_start, h.new_count)));
        out.push(build_hunk(h, opts));
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

fn build_hunk(h: RawHunk, opts: &AlignmentOptions) -> DiffHunk {
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
    for line in h.lines {
        match line.origin {
            Origin::Context => {
                flush(&mut removed, &mut added, &mut rows);
                let new_line = diff_line(line, new_no);
                rows.push(AlignedRow {
                    kind: DiffRowKind::Context,
                    old_line: Some(DiffLine {
                        line_number: old_no,
                        ..new_line.clone()
                    }),
                    new_line: Some(new_line),
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
        header: h.header,
        section_heading: h.section_heading,
        rows,
        alignment: quality,
    }
}

fn diff_line(line: PatchLine, line_number: usize) -> DiffLine {
    let mut content = line.text;
    let ending = if line.no_newline {
        LineEnding::None
    } else if content.last() == Some(&b'\r') {
        content.pop();
        LineEnding::CrLf
    } else {
        LineEnding::Lf
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
