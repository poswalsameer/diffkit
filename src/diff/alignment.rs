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

use crate::types::diff::{AlignedRow, AlignmentOptions, AlignmentQuality, DiffLine, DiffRowKind};
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
        ids: &mut HashMap<(&'a [u8], crate::types::diff::LineEnding), u32>,
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
///
/// Each line's [`Profile`] is computed at most once per block, however many
/// windows it falls in.
fn pair_block(
    old: &mut [Option<DiffLine>],
    new: &mut [Option<DiffLine>],
    dels: &[usize],
    ins: &[usize],
    opts: &AlignmentOptions,
    rows: &mut Vec<AlignedRow>,
) {
    let mut cursor = 0;
    let mut profiles: Vec<Option<Profile>> = Vec::new();
    if !dels.is_empty() {
        profiles.resize_with(ins.len(), || None);
    }
    for &d in dels {
        let window_end = ins.len().min(cursor + opts.pairing_window);
        let mut best: Option<(usize, u8)> = None;
        if let Some(old_line) = old[d].as_ref().filter(|_| cursor < window_end) {
            let a = Profile::new(&old_line.content);
            for (j, &i) in ins.iter().enumerate().take(window_end).skip(cursor) {
                let new_line = &new[i].as_ref().expect("unpaired").content;
                let b = profiles[j].get_or_insert_with(|| Profile::new(new_line));
                let score = a.score(&old_line.content, b, new_line, opts.pairing_threshold);
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
    Profile::new(a).score(a, &Profile::new(b), b, 0)
}

/// What [`similarity`] needs from one line, computed once: where its
/// trimmed content lies, and the sorted bigrams of the compared prefix.
/// The line's bytes are passed again when scoring, so a profile does not
/// borrow its line.
struct Profile {
    trimmed: std::ops::Range<usize>,
    /// Empty when the prefix is too short for a bigram.
    bigrams: Vec<u16>,
}

impl Profile {
    fn new(line: &[u8]) -> Self {
        // The same trimming as `<[u8]>::trim_ascii`, keeping the offsets.
        let start = line
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(line.len());
        let end = line
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(start, |i| i + 1);
        let trimmed = &line[start..end];
        let prefix = &trimmed[..trimmed.len().min(SIMILARITY_PREFIX)];
        let mut bigrams: Vec<u16> = prefix
            .windows(2)
            .map(|w| u16::from_be_bytes([w[0], w[1]]))
            .collect();
        bigrams.sort_unstable();
        Self {
            trimmed: start..end,
            bigrams,
        }
    }

    /// The [`similarity`] of this profile's `line` and `other`'s
    /// `other_line`, except that a score that cannot reach `floor` (judged
    /// from the bigram counts alone) is reported as 0 without counting.
    fn score(&self, line: &[u8], other: &Profile, other_line: &[u8], floor: u8) -> u8 {
        if line[self.trimmed.clone()] == other_line[other.trimmed.clone()] {
            return 100;
        }
        let (x, y) = (&self.bigrams, &other.bigrams);
        if x.is_empty() || y.is_empty() {
            return 0;
        }
        let total = x.len() + y.len();
        if 200 * x.len().min(y.len()) / total < usize::from(floor) {
            return 0;
        }
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
        (200 * common / total) as u8
    }
}
