//! Unified patch parsing from byte fixtures: sections, hunk headers, paths
//! (quoted, renamed, non-UTF-8), and errors with line numbers.

use diffkit::diff::{errors::DiffError, parser::*};

const OID_A: &str = "1111111111111111111111111111111111111111";
const OID_B: &str = "2222222222222222222222222222222222222222";

fn one(input: &[u8]) -> FilePatch {
    let mut v = parse_patch(input).unwrap();
    assert_eq!(v.len(), 1, "{v:#?}");
    v.remove(0)
}

fn texts(h: &RawHunk) -> Vec<(Origin, &[u8], bool)> {
    h.lines
        .iter()
        .map(|l| (l.origin, l.text.as_slice(), l.no_newline))
        .collect()
}

#[test]
fn empty_input_has_no_files() {
    assert!(parse_patch(b"").unwrap().is_empty());
}

#[test]
fn single_file_single_hunk() {
    let input = format!(
        "diff --git a/f.txt b/f.txt\nindex {OID_A}..{OID_B} 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@ fn main\n a\n-b\n+B\n c\n"
    );
    let f = one(input.as_bytes());
    assert_eq!(f.kind, PatchKind::Normal);
    assert_eq!(f.old_path.as_deref(), Some(&b"f.txt"[..]));
    assert_eq!(f.new_path.as_deref(), Some(&b"f.txt"[..]));
    assert_eq!(f.old_oid.as_ref().unwrap().as_str(), OID_A);
    assert_eq!(f.new_oid.as_ref().unwrap().as_str(), OID_B);
    assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100644)));
    let h = &f.hunks[0];
    assert_eq!(
        (h.old_start, h.old_count, h.new_start, h.new_count),
        (1, 3, 1, 3)
    );
    assert_eq!(h.section_heading, b"fn main");
    assert_eq!(
        texts(h),
        vec![
            (Origin::Context, &b"a"[..], false),
            (Origin::Removed, b"b", false),
            (Origin::Added, b"B", false),
            (Origin::Context, b"c", false)
        ]
    );
    assert_eq!(f.counts(), (1, 1));
}

#[test]
fn multiple_files_and_hunks() {
    let input = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-1\n+2\n@@ -10,2 +10,3 @@\n k\n+n\n k\n\
diff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -5 +5,0 @@\n-gone\n";
    let v = parse_patch(input).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].hunks.len(), 2);
    assert_eq!((v[0].hunks[1].old_start, v[0].hunks[1].new_count), (10, 3));
    let h = &v[1].hunks[0];
    assert_eq!(
        (h.old_start, h.old_count, h.new_start, h.new_count),
        (5, 1, 5, 0)
    );
}

/// Parses one hunk with header `h` and a body matching `counts`, returning
/// the parsed start and count of each side; `None` if no hunk was parsed.
fn hunk_counts(h: &str, counts: [usize; 2]) -> Option<[usize; 4]> {
    let context = counts[0].min(counts[1]);
    let body = " c\n".repeat(context)
        + &"-r\n".repeat(counts[0] - context)
        + &"+a\n".repeat(counts[1] - context);
    let patch = format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n{h}\n{body}");
    let files = parse_patch(patch.as_bytes()).ok()?;
    let h = files.first()?.hunks.first()?;
    Some([h.old_start, h.old_count, h.new_start, h.new_count])
}

#[test]
fn hunk_header_forms() {
    assert_eq!(hunk_counts("@@ -1,5 +1,6 @@", [5, 6]), Some([1, 5, 1, 6]));
    assert_eq!(
        hunk_counts("@@ -10 +10 @@", [1, 1]),
        Some([10, 1, 10, 1]),
        "missing count means one line"
    );
    assert_eq!(hunk_counts("@@ -0,0 +1,3 @@", [0, 3]), Some([0, 0, 1, 3]));
    assert_eq!(hunk_counts("@@ -5,3 +5,0 @@", [3, 0]), Some([5, 3, 5, 0]));
    for bad in ["@@ -a +1 @@", "@@ -1 +1", "@@ -1,+1 @@", "@@@ -1 -1 +1 @@@"] {
        assert!(hunk_counts(bad, [1, 1]).is_none(), "{bad:?}");
    }
}

#[test]
fn addition_and_deletion_with_dev_null() {
    let add = one(b"diff --git a/n b/n\nnew file mode 100755\nindex 0000000000000000000000000000000000000000..1111111111111111111111111111111111111111\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+x\n+y\n");
    assert!(add.new_file);
    assert_eq!(add.old_path, None);
    assert_eq!(add.new_path.as_deref(), Some(&b"n"[..]));
    assert_eq!(
        add.old_oid, None,
        "zero object ID is not invented as an object"
    );
    assert_eq!(add.new_mode, Some(0o100755));
    let del = one(b"diff --git a/d b/d\ndeleted file mode 100644\n--- a/d\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n");
    assert!(del.deleted_file);
    assert_eq!(
        (del.old_path.as_deref(), del.new_path.as_deref()),
        (Some(&b"d"[..]), None)
    );
}

#[test]
fn empty_file_addition_has_no_hunks() {
    let f = one(b"diff --git a/e b/e\nnew file mode 100644\nindex 0000000000000000000000000000000000000000..e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\n");
    assert!(f.new_file && f.hunks.is_empty());
    assert_eq!((f.old_path, f.new_path.as_deref()), (None, Some(&b"e"[..])));
}

#[test]
fn rename_and_copy_metadata() {
    let r = one(b"diff --git a/old name b/new name\nsimilarity index 97%\nrename from old name\nrename to new name\n");
    assert!(r.renamed);
    assert_eq!(r.similarity, Some(97));
    assert_eq!(r.old_path.as_deref(), Some(&b"old name"[..]));
    assert_eq!(r.new_path.as_deref(), Some(&b"new name"[..]));
    let c = one(b"diff --git a/src b/dst\nsimilarity index 100%\ncopy from src\ncopy to dst\n");
    assert!(c.copied);
    assert_eq!(
        (c.old_path.as_deref(), c.new_path.as_deref()),
        (Some(&b"src"[..]), Some(&b"dst"[..]))
    );
    let d = one(b"diff --git a/x b/x\ndissimilarity index 60%\n--- a/x\n+++ b/x\n");
    assert_eq!(d.dissimilarity, Some(60));
}

#[test]
fn mode_change_only() {
    let f = one(b"diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
    assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100755)));
    assert_eq!(
        f.old_path.as_deref(),
        Some(&b"run.sh"[..]),
        "symmetric header"
    );
    assert!(f.hunks.is_empty());
}

#[test]
fn binary_notice() {
    let f = one(b"diff --git a/img.png b/img.png\nindex 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\nBinary files a/img.png and b/img.png differ\n");
    assert!(f.binary && f.hunks.is_empty());
    let added = one(b"diff --git a/b.bin b/b.bin\nnew file mode 100644\nBinary files /dev/null and b/b.bin differ\n");
    assert!(added.binary && added.new_file && added.old_path.is_none());
}

#[test]
fn no_newline_markers() {
    let f = one(b"diff --git a/n b/n\n--- a/n\n+++ b/n\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n");
    assert_eq!(
        texts(&f.hunks[0]),
        vec![
            (Origin::Removed, &b"old"[..], true),
            (Origin::Added, b"new", true)
        ]
    );
}

#[test]
fn quoted_paths_and_trailing_tab() {
    let f = one(b"diff --git \"a/t\\tab\\303\\251\" \"b/t\\tab\\303\\251\"\n--- \"a/t\\tab\\303\\251\"\n+++ \"b/t\\tab\\303\\251\"\n@@ -1 +1 @@\n-a\n+b\n");
    assert_eq!(f.new_path.as_deref(), Some("t\tab\u{e9}".as_bytes()));
    let s =
        one(b"diff --git a/sp ace b/sp ace\n--- a/sp ace\t\n+++ b/sp ace\t\n@@ -1 +1 @@\n-a\n+b\n");
    // Quoted names with a space get the tab after the closing quote.
    let q = one(b"diff --git \"a/emoji \\360\\237\\216\\211.txt\" \"b/emoji \\360\\237\\216\\211.txt\"\n--- \"a/emoji \\360\\237\\216\\211.txt\"\t\n+++ \"b/emoji \\360\\237\\216\\211.txt\"\t\n@@ -1 +1 @@\n-a\n+b\n");
    assert_eq!(
        q.new_path.as_deref(),
        Some("emoji \u{1f389}.txt".as_bytes())
    );
    assert_eq!(s.old_path.as_deref(), Some(&b"sp ace"[..]));
    assert_eq!(s.new_path.as_deref(), Some(&b"sp ace"[..]));
    let mixed = one(b"diff --git a/plain \"b/new\\nline\"\nsimilarity index 100%\nrename from plain\nrename to \"new\\nline\"\n");
    assert_eq!(mixed.new_path.as_deref(), Some(&b"new\nline"[..]));
    assert_eq!(mixed.old_path.as_deref(), Some(&b"plain"[..]));
}

#[test]
fn non_utf8_content_and_paths_are_bytes() {
    let f = one(b"diff --git a/raw\xff b/raw\xff\n--- a/raw\xff\n+++ b/raw\xff\n@@ -1 +1 @@\n-\xfe\xfd\n+\xc3\x28\r\n");
    assert_eq!(f.new_path.as_deref(), Some(&b"raw\xff"[..]));
    assert_eq!(f.hunks[0].lines[0].text, b"\xfe\xfd");
    assert_eq!(f.hunks[0].lines[1].text, b"\xc3\x28\r");
}

#[test]
fn content_that_looks_like_headers_stays_content() {
    let f = one(b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n--- a/fake\n++++ b/fake\n-diff --git a/q b/q\n+@@ -1 +1 @@\n");
    let t = texts(&f.hunks[0]);
    assert_eq!(t[0], (Origin::Removed, &b"-- a/fake"[..], false));
    assert_eq!(t[1], (Origin::Added, &b"+++ b/fake"[..], false));
    assert_eq!(t[2], (Origin::Removed, &b"diff --git a/q b/q"[..], false));
    assert_eq!(t[3], (Origin::Added, &b"@@ -1 +1 @@"[..], false));
}

#[test]
fn combined_and_unmerged_sections() {
    let v = parse_patch(b"* Unmerged path c\\303\\251\ndiff --cc f\nindex 1,2..0\n--- a/f\n+++ b/f\n@@@ -1,1 -1,1 +1,3 @@@\n++<<<<<<< HEAD\n+ x\n++>>>>>>> s\ndiff --git a/k b/k\n--- a/k\n+++ b/k\n@@ -1 +1,2 @@\n k\n+n\n").unwrap();
    assert_eq!(v[0].kind, PatchKind::UnmergedNotice);
    assert_eq!(
        v[0].new_path.as_deref(),
        Some(&b"c\\303\\251"[..]),
        "unquoted notice paths are raw"
    );
    assert_eq!(v[1].kind, PatchKind::Combined);
    assert_eq!(v[1].new_path.as_deref(), Some(&b"f"[..]));
    assert!(v[1].hunks.is_empty());
    assert_eq!(v[2].kind, PatchKind::Normal);
    assert_eq!(v[2].hunks[0].lines.len(), 2);
}

#[test]
fn unknown_headers_are_preserved() {
    let f = one(b"diff --git a/x b/x\nsomething new 42\n--- a/x\n+++ b/x\n");
    assert_eq!(f.extended_headers, vec![b"something new 42".to_vec()]);
}

#[test]
fn zero_line_and_empty_context_hunks() {
    let f = one(b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,0 +4,2 @@\n+a\n+\n@@ -9,3 +11,3 @@\n \n-q\n+r\n\n");
    assert_eq!(f.hunks[0].lines[1].text, b"");
    assert_eq!(
        f.hunks[1].lines.len(),
        4,
        "a bare empty line is an empty context line"
    );
}

#[test]
fn malformed_and_truncated_inputs_are_errors() {
    let bad: &[&[u8]] = &[
        b"garbage\n",
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n",
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n a\n-b\n",
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+c\n",
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\nxa\n",
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,1 +1 @@\n-a\n+b\n",
        b"diff --git a/x b/x\nold mode 10064\n",
        b"diff --git a/x b/x\nindex zz..yy\n",
        b"diff --git a/x b/x\nsimilarity index 101%\n",
        b"diff --git a/x b/x\n--- x\n",
        b"diff --git \"a/x b/x\n",
        b"diff --git a/x y b/z w\n",
    ];
    for input in bad {
        assert!(
            parse_patch(input).is_err(),
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn error_reports_line_number() {
    match parse_patch(b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n") {
        Err(DiffError::Parse { line, detail }) => {
            assert_eq!(line, 6);
            assert!(detail.contains("truncated"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
}
