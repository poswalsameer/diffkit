//! Side-by-side alignment: Myers edit scripts (minimal, bounded), line
//! pairing by similarity, and agreement with the reference implementation.

use diffkit::diff::alignment::*;
use diffkit::types::diff::*;

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

/// Bytes of each line `similarity` compares (documented on the function).
const SIMILARITY_PREFIX: usize = 512;

/// The scoring before line profiles were introduced, kept as the reference.
fn reference_similarity(a: &[u8], b: &[u8]) -> u8 {
    let (a, b) = (a.trim_ascii(), b.trim_ascii());
    if a == b {
        return 100;
    }
    let a = &a[..a.len().min(SIMILARITY_PREFIX)];
    let b = &b[..b.len().min(SIMILARITY_PREFIX)];
    if a.len() < 2 || b.len() < 2 {
        return 0;
    }
    let x: Vec<u16> = a
        .windows(2)
        .map(|w| u16::from_be_bytes([w[0], w[1]]))
        .collect();
    let y: Vec<u16> = b
        .windows(2)
        .map(|w| u16::from_be_bytes([w[0], w[1]]))
        .collect();
    let mut common = 0;
    let mut rest = y.clone();
    for v in &x {
        if let Some(k) = rest.iter().position(|w| w == v) {
            rest.remove(k);
            common += 1;
        }
    }
    (200 * common / (x.len() + y.len())) as u8
}

type Numbers = (DiffRowKind, Option<usize>, Option<usize>);

/// Side-by-side rows as `align` produced them before line profiles: the
/// same edit script, with every pair in the window scored from scratch.
fn reference_align(old: &[DiffLine], new: &[DiffLine], opts: &AlignmentOptions) -> Vec<Numbers> {
    let key = |l: &DiffLine| (l.content.clone(), l.ending);
    let (ok, nk): (Vec<_>, Vec<_>) = (old.iter().map(key).collect(), new.iter().map(key).collect());
    let script = myers(&ok, &nk, opts.max_edit_cost);
    let mut rows = Vec::new();
    let (mut dels, mut ins) = (Vec::new(), Vec::new());
    let pair = |dels: &mut Vec<usize>, ins: &mut Vec<usize>, rows: &mut Vec<Numbers>| {
        let mut cursor = 0;
        for &d in dels.iter() {
            let end = ins.len().min(cursor + opts.pairing_window);
            let mut best: Option<(usize, u8)> = None;
            for (j, &i) in ins.iter().enumerate().take(end).skip(cursor) {
                let score = reference_similarity(&old[d].content, &new[i].content);
                if score >= opts.pairing_threshold && best.is_none_or(|(_, s)| score > s) {
                    best = Some((j, score));
                }
            }
            match best {
                Some((j, _)) => {
                    for &i in &ins[cursor..j] {
                        rows.push((DiffRowKind::Added, None, Some(new[i].line_number)));
                    }
                    rows.push((
                        DiffRowKind::Modified,
                        Some(old[d].line_number),
                        Some(new[ins[j]].line_number),
                    ));
                    cursor = j + 1;
                }
                None => rows.push((DiffRowKind::Removed, Some(old[d].line_number), None)),
            }
        }
        for &i in &ins[cursor..] {
            rows.push((DiffRowKind::Added, None, Some(new[i].line_number)));
        }
        dels.clear();
        ins.clear();
    };
    for op in &script.ops {
        match *op {
            EditOp::Equal { old: i, new: j } => {
                pair(&mut dels, &mut ins, &mut rows);
                rows.push((
                    DiffRowKind::Context,
                    Some(old[i].line_number),
                    Some(new[j].line_number),
                ));
            }
            EditOp::Delete { old: i } => dels.push(i),
            EditOp::Insert { new: j } => ins.push(j),
        }
    }
    pair(&mut dels, &mut ins, &mut rows);
    rows
}

#[test]
fn similarity_and_pairing_match_the_reference_implementation() {
    let mut seed: u64 = 0xd1ce;
    let mut next = |m: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % m
    };
    let alphabet = b" \tabcab{}();";
    let line = |next: &mut dyn FnMut(u64) -> u64| -> Vec<u8> {
        let len = match next(4) {
            0 => next(3),
            1 => next(40),
            2 => 500 + next(40),
            _ => next(1200),
        };
        (0..len)
            .map(|_| alphabet[next(alphabet.len() as u64) as usize])
            .collect()
    };
    for _ in 0..3000 {
        let a = line(&mut next);
        let b = if next(3) == 0 {
            a.clone()
        } else {
            line(&mut next)
        };
        assert_eq!(
            similarity(&a, &b),
            reference_similarity(&a, &b),
            "{a:?} {b:?}"
        );
    }
    // Whole blocks, so the skip for pairs that cannot reach the threshold
    // and the reuse of each line's profile are both exercised.
    let numbered = |v: Vec<Vec<u8>>| -> Vec<DiffLine> {
        v.into_iter()
            .enumerate()
            .map(|(i, content)| DiffLine {
                content,
                line_number: i + 1,
                ending: LineEnding::Lf,
            })
            .collect()
    };
    for round in 0..200 {
        let old: Vec<Vec<u8>> = (0..next(30)).map(|_| line(&mut next)).collect();
        let new: Vec<Vec<u8>> = (0..next(30))
            .map(|_| match next(3) {
                0 if !old.is_empty() => old[next(old.len() as u64) as usize].clone(),
                1 if !old.is_empty() => {
                    let mut l = old[next(old.len() as u64) as usize].clone();
                    l.push(b'x');
                    l
                }
                _ => line(&mut next),
            })
            .collect();
        let (old, new) = (numbered(old), numbered(new));
        for threshold in [0, 40, 90] {
            let opts = AlignmentOptions {
                pairing_threshold: threshold,
                ..AlignmentOptions::default()
            };
            let want = reference_align(&old, &new, &opts);
            let (rows, _) = align(old.clone(), new.clone(), &opts);
            let got: Vec<Numbers> = rows
                .iter()
                .map(|r| {
                    (
                        r.kind,
                        r.old_line.as_ref().map(|l| l.line_number),
                        r.new_line.as_ref().map(|l| l.line_number),
                    )
                })
                .collect();
            assert_eq!(got, want, "round {round}, threshold {threshold}");
        }
    }
}
