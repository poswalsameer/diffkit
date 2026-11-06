//! Line alignment for side-by-side rows.
//!
//! [`myers`] computes a shortest edit script with the linear-space variant
//! of Myers' algorithm (divide and conquer on the "middle snake"): O((N+M)·D)
//! time and O(N+M) memory, where D is the edit distance. There is no N×M
//! matrix. Each middle-snake search stops after `max_edit_cost / 2 + 1`
//! diagonals; a sub-range whose edit distance exceeds the bound is emitted
//! as delete-then-insert and the result is marked
//! [`AlignmentQuality::Bounded`]. The bound depends only on the input, so
//! results are deterministic.
//!
//! [`align`] turns the edit script into rows. Equal lines become `Context`
//! rows. Each run of deletions and insertions between two equal lines is
//! paired by the policy documented on [`pair_block`].

use super::models::{AlignedRow, AlignmentOptions, AlignmentQuality, DiffLine, DiffRowKind};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    Equal { old: usize, new: usize },
    Delete { old: usize },
    Insert { new: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditScript {
    pub ops: Vec<EditOp>,
    /// Some sub-range exceeded the edit-cost bound.
    pub bounded: bool,
}

impl EditScript {
    /// Number of deletions plus insertions.
    pub fn cost(&self) -> usize {
        self.ops
            .iter()
            .filter(|o| !matches!(o, EditOp::Equal { .. }))
            .count()
    }
}

/// Shortest edit script from `old` to `new`, bounded by `max_edit_cost`.
pub fn myers<T: PartialEq>(old: &[T], new: &[T], max_edit_cost: usize) -> EditScript {
    let max_d = (old.len() + new.len()).div_ceil(2) + 1;
    let d_limit = max_d.min(max_edit_cost / 2 + 1);
    let mut s = Search {
        old,
        new,
        vf: Diagonals::new(d_limit + 1),
        vb: Diagonals::new(d_limit + 1),
        d_limit,
        ops: Vec::with_capacity(old.len() + new.len()),
        bounded: false,
    };
    s.conquer(0, old.len(), 0, new.len());
    EditScript {
        ops: s.ops,
        bounded: s.bounded,
    }
}

/// Furthest-reaching x per diagonal k, indexed by k in `-n..=n`.
struct Diagonals {
    offset: isize,
    v: Vec<usize>,
}

impl Diagonals {
    fn new(n: usize) -> Self {
        Self {
            offset: n as isize + 1,
            v: vec![0; 2 * n + 3],
        }
    }

    fn get(&self, k: isize) -> usize {
        self.v[(k + self.offset) as usize]
    }

    fn set(&mut self, k: isize, x: usize) {
        self.v[(k + self.offset) as usize] = x;
    }
}

struct Search<'a, T> {
    old: &'a [T],
    new: &'a [T],
    vf: Diagonals,
    vb: Diagonals,
    d_limit: usize,
    ops: Vec<EditOp>,
    bounded: bool,
}

impl<T: PartialEq> Search<'_, T> {
    fn conquer(&mut self, mut o_lo: usize, mut o_hi: usize, mut n_lo: usize, mut n_hi: usize) {
        while o_lo < o_hi && n_lo < n_hi && self.old[o_lo] == self.new[n_lo] {
            self.ops.push(EditOp::Equal {
                old: o_lo,
                new: n_lo,
            });
            o_lo += 1;
            n_lo += 1;
        }
        let mut suffix = 0;
        while o_hi > o_lo && n_hi > n_lo && self.old[o_hi - 1] == self.new[n_hi - 1] {
            o_hi -= 1;
            n_hi -= 1;
            suffix += 1;
        }
        if o_lo == o_hi {
            self.ops
                .extend((n_lo..n_hi).map(|new| EditOp::Insert { new }));
        } else if n_lo == n_hi {
            self.ops
                .extend((o_lo..o_hi).map(|old| EditOp::Delete { old }));
        } else if let Some((x, y)) = self.middle_snake(o_lo, o_hi, n_lo, n_hi) {
            self.conquer(o_lo, x, n_lo, y);
            self.conquer(x, o_hi, y, n_hi);
        } else {
            self.bounded = true;
            self.ops
                .extend((o_lo..o_hi).map(|old| EditOp::Delete { old }));
            self.ops
                .extend((n_lo..n_hi).map(|new| EditOp::Insert { new }));
        }
        self.ops.extend((0..suffix).map(|i| EditOp::Equal {
            old: o_hi + i,
            new: n_hi + i,
        }));
    }

    /// Finds a split point on an optimal path by searching forward from the
    /// top-left and backward from the bottom-right until the paths overlap.
    /// Returns `None` if the edit distance exceeds the search bound.
    fn middle_snake(
        &mut self,
        o_lo: usize,
        o_hi: usize,
        n_lo: usize,
        n_hi: usize,
    ) -> Option<(usize, usize)> {
        let n = o_hi - o_lo;
        let m = n_hi - n_lo;
        let delta = n as isize - m as isize;
        let odd = delta & 1 == 1;
        self.vf.set(1, 0);
        self.vb.set(1, 0);
        let d_max = ((n + m).div_ceil(2) + 1).min(self.d_limit);
        for d in 0..d_max as isize {
            let mut k = d;
            while k >= -d {
                let mut x = if k == -d || (k != d && self.vf.get(k - 1) < self.vf.get(k + 1)) {
                    self.vf.get(k + 1)
                } else {
                    self.vf.get(k - 1) + 1
                };
                let y = x as isize - k;
                let (x0, y0) = (x, y);
                let mut yy = y;
                while x < n
                    && yy >= 0
                    && (yy as usize) < m
                    && self.old[o_lo + x] == self.new[n_lo + yy as usize]
                {
                    x += 1;
                    yy += 1;
                }
                self.vf.set(k, x);
                let inside = x <= n && yy >= 0 && yy as usize <= m;
                if odd && inside && (k - delta).abs() < d && x + self.vb.get(delta - k) >= n {
                    return Some((o_lo + x0, n_lo + y0 as usize));
                }
                k -= 2;
            }
            let mut k = d;
            while k >= -d {
                let mut x = if k == -d || (k != d && self.vb.get(k - 1) < self.vb.get(k + 1)) {
                    self.vb.get(k + 1)
                } else {
                    self.vb.get(k - 1) + 1
                };
                let mut y = x as isize - k;
                while x < n
                    && y >= 0
                    && (y as usize) < m
                    && self.old[o_lo + n - x - 1] == self.new[n_lo + m - y as usize - 1]
                {
                    x += 1;
                    y += 1;
                }
                self.vb.set(k, x);
                let inside = x <= n && y >= 0 && y as usize <= m;
                if !odd && inside && (k - delta).abs() <= d && x + self.vf.get(delta - k) >= n {
                    return Some((o_lo + n - x, n_lo + m - y as usize));
                }
                k -= 2;
            }
        }
        None
    }
}

/// Aligns two line sequences into side-by-side rows. Lines are equal when
/// both content and line ending are equal.
pub fn align(
    old: Vec<DiffLine>,
    new: Vec<DiffLine>,
    opts: &AlignmentOptions,
) -> (Vec<AlignedRow>, AlignmentQuality) {
    let (old_ids, new_ids, shared) = intern(&old, &new);
    let script = if shared {
        myers(&old_ids, &new_ids, opts.max_edit_cost)
    } else {
        // Nothing in common: delete-then-insert is already the shortest
        // script, so the quadratic search is skipped.
        EditScript {
            ops: (0..old.len())
                .map(|old| EditOp::Delete { old })
                .chain((0..new.len()).map(|new| EditOp::Insert { new }))
                .collect(),
            bounded: false,
        }
    };
    let mut old: Vec<Option<DiffLine>> = old.into_iter().map(Some).collect();
    let mut new: Vec<Option<DiffLine>> = new.into_iter().map(Some).collect();
    let mut rows = Vec::with_capacity(old.len().max(new.len()));
    let (mut dels, mut ins) = (Vec::new(), Vec::new());
    for op in &script.ops {
        match *op {
            EditOp::Equal { old: i, new: j } => {
                pair_block(&mut old, &mut new, &dels, &ins, opts, &mut rows);
                dels.clear();
                ins.clear();
                rows.push(AlignedRow {
                    kind: DiffRowKind::Context,
                    old_line: old[i].take(),
                    new_line: new[j].take(),
                });
            }
            EditOp::Delete { old: i } => dels.push(i),
            EditOp::Insert { new: j } => ins.push(j),
        }
    }
    pair_block(&mut old, &mut new, &dels, &ins, opts, &mut rows);
    let quality = if script.bounded {
        AlignmentQuality::Bounded
    } else {
        AlignmentQuality::Exact
    };
    (rows, quality)
}

/// Maps each distinct (content, ending) to a small integer so the edit
/// search compares integers. Also reports whether any line is shared.
fn intern(old: &[DiffLine], new: &[DiffLine]) -> (Vec<u32>, Vec<u32>, bool) {
    fn id<'a>(
        ids: &mut HashMap<(&'a [u8], super::models::LineEnding), u32>,
        l: &'a DiffLine,
    ) -> u32 {
        let next = ids.len() as u32;
        *ids.entry((l.content.as_slice(), l.ending)).or_insert(next)
    }
    let mut ids = HashMap::new();
    let old_ids: Vec<u32> = old.iter().map(|l| id(&mut ids, l)).collect();
    let old_distinct = ids.len() as u32;
    let new_ids: Vec<u32> = new.iter().map(|l| id(&mut ids, l)).collect();
    let shared = new_ids.iter().any(|&i| i < old_distinct);
    (old_ids, new_ids, shared)
}

/// Pairs one block of deleted lines (`dels`, in order) with the inserted
/// lines that follow it (`ins`):
///
/// 1. Walk the deleted lines in order. For each, look at the next
///    `pairing_window` unpaired inserted lines, at or after the last pairing.
/// 2. Pick the inserted line with the highest similarity (see
///    [`similarity`]); ties go to the earliest. Pair them as `Modified` only if
///    the score is at least `pairing_threshold`. Inserted lines skipped over
///    become `Added` rows before the pair.
/// 3. A deleted line with no qualifying partner becomes a `Removed` row.
///    Leftover inserted lines become `Added` rows at the end.
///
/// Both sides stay in order, so line numbers increase down each column.
fn pair_block(
    old: &mut [Option<DiffLine>],
    new: &mut [Option<DiffLine>],
    dels: &[usize],
    ins: &[usize],
    opts: &AlignmentOptions,
    rows: &mut Vec<AlignedRow>,
) {
    let mut cursor = 0;
    for &d in dels {
        let window_end = ins.len().min(cursor + opts.pairing_window);
        let mut best: Option<(usize, u8)> = None;
        if let Some(old_line) = &old[d] {
            for (j, &i) in ins.iter().enumerate().take(window_end).skip(cursor) {
                let score = similarity(
                    &old_line.content,
                    &new[i].as_ref().expect("unpaired").content,
                );
                if score >= opts.pairing_threshold && best.is_none_or(|(_, s)| score > s) {
                    best = Some((j, score));
                }
            }
        }
        match best {
            Some((j, _)) => {
                for &i in &ins[cursor..j] {
                    rows.push(added(new[i].take()));
                }
                rows.push(AlignedRow {
                    kind: DiffRowKind::Modified,
                    old_line: old[d].take(),
                    new_line: new[ins[j]].take(),
                });
                cursor = j + 1;
            }
            None => rows.push(AlignedRow {
                kind: DiffRowKind::Removed,
                old_line: old[d].take(),
                new_line: None,
            }),
        }
    }
    for &i in &ins[cursor..] {
        rows.push(added(new[i].take()));
    }
}

fn added(line: Option<DiffLine>) -> AlignedRow {
    AlignedRow {
        kind: DiffRowKind::Added,
        old_line: None,
        new_line: line,
    }
}

/// Bytes of each line considered for similarity, to bound the cost on very
/// long lines (minified files).
const SIMILARITY_PREFIX: usize = 512;

/// Similarity of two lines in percent: the Sørensen–Dice coefficient of
/// their byte bigrams after trimming ASCII whitespace at both ends, using at
/// most the first 512 bytes. Identical trimmed lines score 100. Lines too
/// short for a bigram score 0 unless identical.
pub fn similarity(a: &[u8], b: &[u8]) -> u8 {
    let a = a.trim_ascii();
    let b = b.trim_ascii();
    if a == b {
        return 100;
    }
    let a = &a[..a.len().min(SIMILARITY_PREFIX)];
    let b = &b[..b.len().min(SIMILARITY_PREFIX)];
    if a.len() < 2 || b.len() < 2 {
        return 0;
    }
    let bigrams = |s: &[u8]| {
        let mut v: Vec<u16> = s
            .windows(2)
            .map(|w| u16::from_be_bytes([w[0], w[1]]))
            .collect();
        v.sort_unstable();
        v
    };
    let (x, y) = (bigrams(a), bigrams(b));
    let (mut i, mut j, mut common) = (0, 0, 0usize);
    while i < x.len() && j < y.len() {
        match x[i].cmp(&y[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    (200 * common / (x.len() + y.len())) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::models::LineEnding;

    fn lines(texts: &[&str]) -> Vec<DiffLine> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| DiffLine {
                content: t.as_bytes().to_vec(),
                line_number: i + 1,
                ending: LineEnding::Lf,
            })
            .collect()
    }

    /// (kind, old line number, new line number, old text, new text)
    type Row = (DiffRowKind, Option<usize>, Option<usize>, String, String);

    fn summary(rows: &[AlignedRow]) -> Vec<Row> {
        let text = |l: &Option<DiffLine>| {
            l.as_ref()
                .map(|l| String::from_utf8_lossy(&l.content).into_owned())
                .unwrap_or_default()
        };
        rows.iter()
            .map(|r| {
                (
                    r.kind,
                    r.old_line.as_ref().map(|l| l.line_number),
                    r.new_line.as_ref().map(|l| l.line_number),
                    text(&r.old_line),
                    text(&r.new_line),
                )
            })
            .collect()
    }

    fn run(old: &[&str], new: &[&str]) -> Vec<Row> {
        let (rows, q) = align(lines(old), lines(new), &AlignmentOptions::default());
        assert_eq!(q, AlignmentQuality::Exact);
        assert_rows_consistent(&rows, old.len(), new.len());
        summary(&rows)
    }

    /// Every old and new line appears exactly once, in order, on its side.
    fn assert_rows_consistent(rows: &[AlignedRow], n_old: usize, n_new: usize) {
        let olds: Vec<usize> = rows
            .iter()
            .filter_map(|r| r.old_line.as_ref().map(|l| l.line_number))
            .collect();
        let news: Vec<usize> = rows
            .iter()
            .filter_map(|r| r.new_line.as_ref().map(|l| l.line_number))
            .collect();
        assert_eq!(olds, (1..=n_old).collect::<Vec<_>>());
        assert_eq!(news, (1..=n_new).collect::<Vec<_>>());
        for r in rows {
            let ok = match r.kind {
                DiffRowKind::Context | DiffRowKind::Modified => {
                    r.old_line.is_some() && r.new_line.is_some()
                }
                DiffRowKind::Removed => r.old_line.is_some() && r.new_line.is_none(),
                DiffRowKind::Added => r.old_line.is_none() && r.new_line.is_some(),
                DiffRowKind::Empty => false,
            };
            assert!(ok, "{r:?}");
            if r.kind == DiffRowKind::Context {
                assert_eq!(
                    r.old_line.as_ref().unwrap().content,
                    r.new_line.as_ref().unwrap().content
                );
            }
        }
    }

    use DiffRowKind::*;

    fn row(k: DiffRowKind, o: Option<usize>, n: Option<usize>, ot: &str, nt: &str) -> Row {
        (k, o, n, ot.into(), nt.into())
    }

    #[test]
    fn single_line_replacement_is_a_modified_row() {
        let got = run(
            &[
                "const name = \"Alice\";",
                "const age = 20;",
                "console.log(name);",
            ],
            &[
                "const name = \"Alice\";",
                "const age = 21;",
                "console.log(name);",
            ],
        );
        assert_eq!(
            got,
            vec![
                row(
                    Context,
                    Some(1),
                    Some(1),
                    "const name = \"Alice\";",
                    "const name = \"Alice\";"
                ),
                row(
                    Modified,
                    Some(2),
                    Some(2),
                    "const age = 20;",
                    "const age = 21;"
                ),
                row(
                    Context,
                    Some(3),
                    Some(3),
                    "console.log(name);",
                    "console.log(name);"
                ),
            ]
        );
    }

    #[test]
    fn multiple_line_replacement() {
        let got = run(
            &["a", "let x = 1;", "let y = 2;", "z"],
            &["a", "let x = 10;", "let y = 20;", "z"],
        );
        assert_eq!(got[1].0, Modified);
        assert_eq!(got[2].0, Modified);
        assert_eq!((got[2].1, got[2].2), (Some(3), Some(3)));
    }

    #[test]
    fn insertion_only_and_deletion_only() {
        assert_eq!(
            run(&["a", "c"], &["a", "b", "c"]),
            vec![
                row(Context, Some(1), Some(1), "a", "a"),
                row(Added, None, Some(2), "", "b"),
                row(Context, Some(2), Some(3), "c", "c"),
            ]
        );
        assert_eq!(
            run(&["a", "b", "c"], &["a", "c"]),
            vec![
                row(Context, Some(1), Some(1), "a", "a"),
                row(Removed, Some(2), None, "b", ""),
                row(Context, Some(3), Some(2), "c", "c"),
            ]
        );
    }

    #[test]
    fn unrelated_lines_are_not_paired() {
        let got = run(&["fn alpha() {}"], &["struct Zed;"]);
        assert_eq!(
            got,
            vec![
                row(Removed, Some(1), None, "fn alpha() {}", ""),
                row(Added, None, Some(1), "", "struct Zed;"),
            ]
        );
    }

    #[test]
    fn reordering() {
        let got = run(
            &["first line", "second line"],
            &["second line", "first line"],
        );
        let kinds: Vec<_> = got.iter().map(|r| r.0).collect();
        assert_eq!(kinds.iter().filter(|k| **k == Context).count(), 1);
        assert_eq!(kinds.len(), 3);
    }

    #[test]
    fn repeated_lines_are_deterministic() {
        let old = ["}", "}", "x", "}", "}"];
        let new = ["}", "x", "}", "y", "}", "}"];
        let first = run(&old, &new);
        for _ in 0..5 {
            assert_eq!(run(&old, &new), first);
        }
        let script = myers(&old, &new, usize::MAX);
        assert_eq!(script.cost(), edit_distance(&old, &new));
    }

    #[test]
    fn empty_lines_and_line_endings() {
        let got = run(&["a", "", "b"], &["a", "", "", "b"]);
        assert_eq!(got.iter().filter(|r| r.0 == Added).count(), 1);
        let mut crlf = lines(&["x"]);
        crlf[0].ending = LineEnding::CrLf;
        let (rows, _) = align(lines(&["x"]), crlf, &AlignmentOptions::default());
        assert_eq!(
            rows.len(),
            1,
            "same text, different ending: one Modified row"
        );
        assert_eq!(rows[0].kind, Modified);
    }

    #[test]
    fn identical_and_empty_inputs() {
        let got = run(&["a", "b"], &["a", "b"]);
        assert!(got.iter().all(|r| r.0 == Context));
        assert!(run(&[], &[]).is_empty());
        assert_eq!(run(&[], &["n"]), vec![row(Added, None, Some(1), "", "n")]);
        assert_eq!(run(&["o"], &[]), vec![row(Removed, Some(1), None, "o", "")]);
    }

    #[test]
    fn insertion_before_a_modification_does_not_shift_pairs() {
        let got = run(
            &["let total = a + b;"],
            &["// new comment", "let total = a + b + c;"],
        );
        assert_eq!(
            got,
            vec![
                row(Added, None, Some(1), "", "// new comment"),
                row(
                    Modified,
                    Some(1),
                    Some(2),
                    "let total = a + b;",
                    "let total = a + b + c;"
                ),
            ]
        );
    }

    #[test]
    fn no_common_lines_large_block_skips_search() {
        let old: Vec<String> = (0..20_000).map(|i| format!("old {i}")).collect();
        let new: Vec<String> = (0..20_000).map(|i| format!("new {i} different")).collect();
        let o: Vec<&str> = old.iter().map(String::as_str).collect();
        let n: Vec<&str> = new.iter().map(String::as_str).collect();
        let t = std::time::Instant::now();
        let (rows, q) = align(lines(&o), lines(&n), &AlignmentOptions::default());
        assert_eq!(q, AlignmentQuality::Exact);
        assert_rows_consistent(&rows, 20_000, 20_000);
        assert!(t.elapsed().as_secs() < 10);
    }

    #[test]
    fn edit_cost_bound_is_explicit_and_deterministic() {
        // Interleaved common lines force a large edit distance.
        let old: Vec<u32> = (0..4000).map(|i| if i % 2 == 0 { 0 } else { i }).collect();
        let new: Vec<u32> = (0..4000)
            .map(|i| if i % 2 == 0 { 0 } else { i + 100_000 })
            .collect();
        let full = myers(&old, &new, usize::MAX);
        assert!(!full.bounded);
        assert_eq!(full.cost(), 4000);
        let limited = myers(&old, &new, 100);
        assert!(limited.bounded);
        assert_eq!(limited, myers(&old, &new, 100), "deterministic");
        assert_script_valid(&old, &new, &limited);
    }

    #[test]
    fn large_block_with_common_lines_completes() {
        let old: Vec<String> = (0..3000).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        for i in (0..3000).step_by(7) {
            new[i] = format!("changed {i}");
        }
        let o: Vec<&str> = old.iter().map(String::as_str).collect();
        let n: Vec<&str> = new.iter().map(String::as_str).collect();
        let (rows, q) = align(lines(&o), lines(&n), &AlignmentOptions::default());
        assert_eq!(q, AlignmentQuality::Exact);
        assert_rows_consistent(&rows, 3000, 3000);
        assert_eq!(
            rows.iter().filter(|r| r.kind == Context).count(),
            3000 - 429
        );
    }

    fn assert_script_valid<T: PartialEq + std::fmt::Debug>(old: &[T], new: &[T], s: &EditScript) {
        let (mut i, mut j) = (0, 0);
        for op in &s.ops {
            match *op {
                EditOp::Equal { old: o, new: n } => {
                    assert_eq!((o, n), (i, j));
                    assert_eq!(old[o], new[n]);
                    i += 1;
                    j += 1;
                }
                EditOp::Delete { old: o } => {
                    assert_eq!(o, i);
                    i += 1;
                }
                EditOp::Insert { new: n } => {
                    assert_eq!(n, j);
                    j += 1;
                }
            }
        }
        assert_eq!((i, j), (old.len(), new.len()));
    }

    /// O(N·M) reference used only to check minimality in tests.
    fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i; b.len() + 1];
            for j in 1..=b.len() {
                cur[j] = if a[i - 1] == b[j - 1] {
                    prev[j - 1]
                } else {
                    1 + prev[j].min(cur[j - 1])
                };
            }
            prev = cur;
        }
        prev[b.len()]
    }

    #[test]
    fn myers_is_minimal_on_random_inputs() {
        let mut seed: u64 = 0x5eed;
        let mut next = |m: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        for _ in 0..2000 {
            let alphabet = next(4) + 1;
            let a: Vec<u64> = (0..next(30)).map(|_| next(alphabet)).collect();
            let b: Vec<u64> = (0..next(30)).map(|_| next(alphabet)).collect();
            let s = myers(&a, &b, usize::MAX);
            assert_script_valid(&a, &b, &s);
            assert_eq!(s.cost(), edit_distance(&a, &b), "{a:?} {b:?}");
            assert!(!s.bounded);
        }
    }

    #[test]
    fn similarity_scores() {
        assert_eq!(similarity(b"  same  ", b"same"), 100);
        assert!(similarity(b"const age = 20;", b"const age = 21;") >= 80);
        assert!(similarity(b"fn alpha() {}", b"struct Zed;") < 40);
        assert_eq!(similarity(b"", b"x"), 0);
        assert_eq!(similarity(b"", b""), 100);
        let long_a = vec![b'a'; 100_000];
        let long_b = vec![b'a'; 90_000];
        assert_eq!(
            similarity(&long_a, &long_b),
            100,
            "compares the first 512 bytes"
        );
    }
}
