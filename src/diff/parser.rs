//! Byte-oriented parser for Git's unified diff output.
//!
//! The output is split into lines on `\n` only. File content never contains
//! `\n` inside a line, and Git C-quotes header paths that contain control
//! characters, so line splitting is safe. Paths and content are kept as
//! bytes; nothing is assumed to be UTF-8.
//!
//! Hunk bodies are read by count: exactly as many old and new lines as the
//! `@@` header declares, so a truncated or miscounted hunk is an error
//! instead of being silently shortened or extended.

use super::errors::{DiffError, Result};
use crate::git::models::ObjectId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchKind {
    /// `diff --git` section.
    Normal,
    /// `diff --cc` / `diff --combined` section (an unmerged path). Its body
    /// is skipped; Diffkit does not present combined diffs as two-way diffs.
    Combined,
    /// `* Unmerged path <path>` notice.
    UnmergedNotice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Context,
    Removed,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLine {
    pub origin: Origin,
    /// The line without the origin character and without the patch's `\n`.
    pub text: Vec<u8>,
    /// Followed by `\ No newline at end of file`.
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: Vec<u8>,
    pub section_heading: Vec<u8>,
    pub lines: Vec<PatchLine>,
    /// 1-based line of the `@@` header in the patch, for error messages.
    pub patch_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    pub kind: PatchKind,
    /// The section's first line, verbatim.
    pub header: Vec<u8>,
    /// Resolved paths without `a/`/`b/` prefixes; `None` is `/dev/null`.
    pub old_path: Option<Vec<u8>>,
    pub new_path: Option<Vec<u8>>,
    pub old_mode: Option<u32>,
    pub new_mode: Option<u32>,
    pub new_file: bool,
    pub deleted_file: bool,
    pub similarity: Option<u8>,
    pub dissimilarity: Option<u8>,
    pub renamed: bool,
    pub copied: bool,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub binary: bool,
    pub hunks: Vec<RawHunk>,
    /// Header lines Diffkit does not interpret, kept verbatim.
    pub extended_headers: Vec<Vec<u8>>,
}

impl FilePatch {
    fn new(kind: PatchKind, header: &[u8]) -> Self {
        Self {
            kind,
            header: header.to_vec(),
            old_path: None,
            new_path: None,
            old_mode: None,
            new_mode: None,
            new_file: false,
            deleted_file: false,
            similarity: None,
            dissimilarity: None,
            renamed: false,
            copied: false,
            old_oid: None,
            new_oid: None,
            binary: false,
            hunks: Vec::new(),
            extended_headers: Vec::new(),
        }
    }

    /// Added and removed line counts over all hunks.
    pub fn counts(&self) -> (u64, u64) {
        let mut added = 0;
        let mut removed = 0;
        for line in self.hunks.iter().flat_map(|h| &h.lines) {
            match line.origin {
                Origin::Added => added += 1,
                Origin::Removed => removed += 1,
                Origin::Context => {}
            }
        }
        (added, removed)
    }
}

pub fn parse_patch(input: &[u8]) -> Result<Vec<FilePatch>> {
    let mut lines: Vec<&[u8]> = input.split(|&b| b == b'\n').collect();
    if input.is_empty() || input.ends_with(b"\n") {
        lines.pop();
    }
    let mut p = Parser { lines, pos: 0 };
    let mut out = Vec::new();
    while let Some(line) = p.peek() {
        if let Some(rest) = line.strip_prefix(b"diff --git ") {
            out.push(p.normal(rest)?);
        } else if let Some(rest) = line
            .strip_prefix(b"diff --cc ")
            .or_else(|| line.strip_prefix(b"diff --combined "))
        {
            out.push(p.combined(rest)?);
        } else if let Some(rest) = line.strip_prefix(b"* Unmerged path ") {
            let path = path_token(rest).ok_or_else(|| p.error("unreadable unmerged path"))?;
            let mut f = FilePatch::new(PatchKind::UnmergedNotice, line);
            f.old_path = Some(path.clone());
            f.new_path = Some(path);
            p.pos += 1;
            out.push(f);
        } else {
            return Err(p.error(format!("expected a file header, found {}", show(line))));
        }
    }
    Ok(out)
}

fn is_section_start(line: &[u8]) -> bool {
    line.starts_with(b"diff --git ")
        || line.starts_with(b"diff --cc ")
        || line.starts_with(b"diff --combined ")
        || line.starts_with(b"* Unmerged path ")
}

/// A `---`/`+++` path; `None` is `/dev/null`.
type SidePath = Option<Vec<u8>>;

struct Parser<'a> {
    lines: Vec<&'a [u8]>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a [u8]> {
        self.lines.get(self.pos).copied()
    }

    /// Error at the current line (1-based).
    fn error(&self, detail: impl Into<String>) -> DiffError {
        DiffError::parse(self.pos + 1, detail)
    }

    fn combined(&mut self, rest: &[u8]) -> Result<FilePatch> {
        let header = self.lines[self.pos];
        let path = path_token(rest).ok_or_else(|| self.error("unreadable combined-diff path"))?;
        let mut f = FilePatch::new(PatchKind::Combined, header);
        f.old_path = Some(path.clone());
        f.new_path = Some(path);
        self.pos += 1;
        while self.peek().is_some_and(|l| !is_section_start(l)) {
            self.pos += 1;
        }
        Ok(f)
    }

    fn normal(&mut self, rest: &[u8]) -> Result<FilePatch> {
        let mut f = FilePatch::new(PatchKind::Normal, self.lines[self.pos]);
        let header_paths = git_header_paths(rest);
        self.pos += 1;
        // Outer None: no `---`/`+++` line; inner None: `/dev/null`.
        let mut minus: Option<SidePath> = None;
        let mut plus: Option<SidePath> = None;
        let (mut from, mut to): (Option<Vec<u8>>, Option<Vec<u8>>) = (None, None);
        while let Some(line) = self.peek() {
            if line.starts_with(b"@@ ") || is_section_start(line) {
                break;
            }
            if let Some(v) = line.strip_prefix(b"old mode ") {
                f.old_mode = Some(self.mode(v)?);
            } else if let Some(v) = line.strip_prefix(b"new mode ") {
                f.new_mode = Some(self.mode(v)?);
            } else if let Some(v) = line.strip_prefix(b"deleted file mode ") {
                f.deleted_file = true;
                f.old_mode = Some(self.mode(v)?);
            } else if let Some(v) = line.strip_prefix(b"new file mode ") {
                f.new_file = true;
                f.new_mode = Some(self.mode(v)?);
            } else if let Some(v) = line.strip_prefix(b"similarity index ") {
                f.similarity = Some(self.percent(v)?);
            } else if let Some(v) = line.strip_prefix(b"dissimilarity index ") {
                f.dissimilarity = Some(self.percent(v)?);
            } else if let Some(v) = line.strip_prefix(b"rename from ") {
                f.renamed = true;
                from = Some(path_token(v).ok_or_else(|| self.error("bad rename path"))?);
            } else if let Some(v) = line.strip_prefix(b"rename to ") {
                f.renamed = true;
                to = Some(path_token(v).ok_or_else(|| self.error("bad rename path"))?);
            } else if let Some(v) = line.strip_prefix(b"copy from ") {
                f.copied = true;
                from = Some(path_token(v).ok_or_else(|| self.error("bad copy path"))?);
            } else if let Some(v) = line.strip_prefix(b"copy to ") {
                f.copied = true;
                to = Some(path_token(v).ok_or_else(|| self.error("bad copy path"))?);
            } else if let Some(v) = line.strip_prefix(b"index ") {
                self.index_line(&mut f, v)?;
            } else if let Some(v) = line.strip_prefix(b"--- ") {
                minus = Some(side_path(v, b"a/").ok_or_else(|| self.error("bad --- path"))?);
            } else if let Some(v) = line.strip_prefix(b"+++ ") {
                plus = Some(side_path(v, b"b/").ok_or_else(|| self.error("bad +++ path"))?);
            } else if line.starts_with(b"Binary files ") && line.ends_with(b" differ") {
                f.binary = true;
            } else if line == b"GIT binary patch" {
                // Only produced with --binary, which Diffkit never passes.
                f.binary = true;
                f.extended_headers.push(line.to_vec());
                while self.peek().is_some_and(|l| !is_section_start(l)) {
                    self.pos += 1;
                }
                break;
            } else {
                f.extended_headers.push(line.to_vec());
            }
            self.pos += 1;
        }
        while self.peek().is_some_and(|l| l.starts_with(b"@@ ")) {
            let hunk = self.hunk()?;
            f.hunks.push(hunk);
        }
        if let Some(line) = self.peek() {
            if !is_section_start(line) {
                return Err(self.error(format!(
                    "unexpected line after hunk (more lines than the header declares?): {}",
                    show(line)
                )));
            }
        }
        let (h_old, h_new) = match header_paths {
            Some((o, n)) => (Some(o), Some(n)),
            None => (None, None),
        };
        f.old_path = match (from, minus) {
            (Some(p), _) => Some(p),
            (None, Some(m)) => m,
            (None, None) if f.new_file => None,
            (None, None) => h_old,
        };
        f.new_path = match (to, plus) {
            (Some(p), _) => Some(p),
            (None, Some(p)) => p,
            (None, None) if f.deleted_file => None,
            (None, None) => h_new,
        };
        if f.old_path.is_none() && f.new_path.is_none() {
            return Err(DiffError::parse(
                self.pos,
                format!("cannot determine the paths of {}", show(&f.header)),
            ));
        }
        Ok(f)
    }

    fn mode(&self, v: &[u8]) -> Result<u32> {
        parse_mode(v).ok_or_else(|| self.error(format!("bad mode {}", show(v))))
    }

    fn percent(&self, v: &[u8]) -> Result<u8> {
        v.strip_suffix(b"%")
            .and_then(|d| std::str::from_utf8(d).ok())
            .and_then(|d| d.parse::<u8>().ok())
            .filter(|&p| p <= 100)
            .ok_or_else(|| self.error(format!("bad percentage {}", show(v))))
    }

    /// `index <old>..<new>[ <mode>]`
    fn index_line(&self, f: &mut FilePatch, v: &[u8]) -> Result<()> {
        let bad = || self.error(format!("bad index line {}", show(v)));
        let (ids, mode) = match v.iter().position(|&b| b == b' ') {
            Some(i) => (&v[..i], Some(&v[i + 1..])),
            None => (v, None),
        };
        let sep = ids.windows(2).position(|w| w == b"..").ok_or_else(bad)?;
        let oid = |b: &[u8]| {
            ObjectId::parse(b)
                .ok_or_else(bad)
                .map(|o| (!o.is_zero()).then_some(o))
        };
        f.old_oid = oid(&ids[..sep])?;
        f.new_oid = oid(&ids[sep + 2..])?;
        if let Some(m) = mode {
            let m = self.mode(m)?;
            f.old_mode.get_or_insert(m);
            f.new_mode.get_or_insert(m);
        }
        Ok(())
    }

    fn hunk(&mut self) -> Result<RawHunk> {
        let patch_line = self.pos + 1;
        let header = self.lines[self.pos];
        let (old_start, old_count, new_start, new_count, heading) = parse_hunk_header(header)
            .ok_or_else(|| self.error(format!("bad hunk header {}", show(header))))?;
        if (old_count > 0 && old_start == 0) || (new_count > 0 && new_start == 0) {
            return Err(self.error("hunk with lines cannot start at line 0"));
        }
        self.pos += 1;
        let (mut old_left, mut new_left) = (old_count, new_count);
        let mut lines = Vec::with_capacity(old_count.max(new_count));
        while old_left > 0 || new_left > 0 {
            let Some(line) = self.peek() else {
                return Err(self.error(format!(
                    "truncated hunk: {old_left} old and {new_left} new lines missing"
                )));
            };
            let (origin, text) = match line.first() {
                Some(b' ') => (Origin::Context, &line[1..]),
                // Some tools strip the space of an empty context line.
                None => (Origin::Context, line),
                Some(b'-') => (Origin::Removed, &line[1..]),
                Some(b'+') => (Origin::Added, &line[1..]),
                _ => {
                    return Err(self.error(format!(
                    "hunk ended early: {old_left} old and {new_left} new lines missing before {}",
                    show(line)
                )))
                }
            };
            let (needs_old, needs_new) = match origin {
                Origin::Context => (true, true),
                Origin::Removed => (true, false),
                Origin::Added => (false, true),
            };
            if (needs_old && old_left == 0) || (needs_new && new_left == 0) {
                return Err(self.error("hunk has more lines than its header declares"));
            }
            old_left -= needs_old as usize;
            new_left -= needs_new as usize;
            self.pos += 1;
            let no_newline = self.peek().is_some_and(|l| l.starts_with(b"\\"));
            if no_newline {
                self.pos += 1;
            }
            lines.push(PatchLine {
                origin,
                text: text.to_vec(),
                no_newline,
            });
        }
        Ok(RawHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            header: header.to_vec(),
            section_heading: heading.to_vec(),
            lines,
            patch_line,
        })
    }
}

fn parse_mode(v: &[u8]) -> Option<u32> {
    std::str::from_utf8(v)
        .ok()
        .filter(|s| s.len() == 6)
        .and_then(|s| u32::from_str_radix(s, 8).ok())
}

/// Parses `@@ -a[,b] +c[,d] @@[ heading]`. A missing count means 1.
fn parse_hunk_header(line: &[u8]) -> Option<(usize, usize, usize, usize, &[u8])> {
    let rest = line.strip_prefix(b"@@ -")?;
    let (old_start, old_count, rest) = range(rest)?;
    let rest = rest.strip_prefix(b" +")?;
    let (new_start, new_count, rest) = range(rest)?;
    let rest = rest.strip_prefix(b" @@")?;
    let heading = rest.strip_prefix(b" ").unwrap_or(rest);
    Some((old_start, old_count, new_start, new_count, heading))
}

fn range(s: &[u8]) -> Option<(usize, usize, &[u8])> {
    let (start, rest) = number(s)?;
    match rest.strip_prefix(b",") {
        Some(r) => {
            let (count, rest) = number(r)?;
            Some((start, count, rest))
        }
        None => Some((start, 1, rest)),
    }
}

fn number(s: &[u8]) -> Option<(usize, &[u8])> {
    let end = s
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(s.len());
    let n = std::str::from_utf8(&s[..end]).ok()?.parse().ok()?;
    Some((n, &s[end..]))
}

/// Decodes a C-quoted string starting at `s[0] == '"'`. Returns the bytes
/// and the number of input bytes consumed, including both quotes.
pub fn unquote(s: &[u8]) -> Option<(Vec<u8>, usize)> {
    if s.first() != Some(&b'"') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < s.len() {
        match s[i] {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                let c = *s.get(i + 1)?;
                i += 2;
                out.push(match c {
                    b'a' => 7,
                    b'b' => 8,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 11,
                    b'f' => 12,
                    b'r' => b'\r',
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'0'..=b'3' => {
                        let d = s.get(i..i + 2)?;
                        if !d.iter().all(|b| (b'0'..=b'7').contains(b)) {
                            return None;
                        }
                        i += 2;
                        (c - b'0') * 64 + (d[0] - b'0') * 8 + (d[1] - b'0')
                    }
                    _ => return None,
                });
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    None
}

/// A path that is either C-quoted (and must span the whole value) or raw.
fn path_token(v: &[u8]) -> Option<Vec<u8>> {
    if v.first() == Some(&b'"') {
        let (p, used) = unquote(v)?;
        (used == v.len()).then_some(p)
    } else {
        (!v.is_empty()).then(|| v.to_vec())
    }
}

/// `--- a/path`, `+++ b/path`, or `/dev/null` (→ `Some(None)`). Git appends a
/// tab to names containing a space, after the closing quote if the name is
/// quoted. A name never ends in a real tab: that would have been quoted.
fn side_path(v: &[u8], prefix: &[u8]) -> Option<Option<Vec<u8>>> {
    if v == b"/dev/null" {
        return Some(None);
    }
    let v = v.strip_suffix(b"\t").unwrap_or(v);
    let raw = if v.first() == Some(&b'"') {
        path_token(v)?
    } else {
        v.to_vec()
    };
    raw.strip_prefix(prefix).map(|p| Some(p.to_vec()))
}

/// Best-effort paths from `diff --git a/<old> b/<new>`. Unquoted names with
/// spaces are ambiguous unless both names are equal; `---`/`+++` and
/// rename/copy lines take precedence whenever they exist.
fn git_header_paths(rest: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let strip = |p: Vec<u8>, prefix: &[u8]| p.strip_prefix(prefix).map(<[u8]>::to_vec);
    if rest.first() == Some(&b'"') {
        let (old, used) = unquote(rest)?;
        let second = rest[used..].strip_prefix(b" ")?;
        let new = path_token(second)?;
        return Some((strip(old, b"a/")?, strip(new, b"b/")?));
    }
    if rest.ends_with(b"\"") {
        for i in (0..rest.len()).filter(|&i| rest[i..].starts_with(b" \"")) {
            if let Some((new, used)) = unquote(&rest[i + 1..]) {
                if i + 1 + used == rest.len() {
                    return Some((strip(rest[..i].to_vec(), b"a/")?, strip(new, b"b/")?));
                }
            }
        }
        return None;
    }
    // Symmetric form: "a/P b/P".
    if rest.len() < 5 || !(rest.len() - 1).is_multiple_of(2) {
        return None;
    }
    let half = (rest.len() - 1) / 2;
    if rest[half] != b' ' {
        return None;
    }
    let old = rest[..half].strip_prefix(b"a/")?;
    let new = rest[half + 1..].strip_prefix(b"b/")?;
    (old == new).then(|| (old.to_vec(), new.to_vec()))
}

fn show(bytes: &[u8]) -> String {
    let cut = &bytes[..bytes.len().min(120)];
    format!("{:?}", String::from_utf8_lossy(cut))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn hunk_header_forms() {
        let nums = |h: &[u8]| {
            let (a, b, c, d, _) = parse_hunk_header(h).unwrap();
            [a, b, c, d]
        };
        assert_eq!(nums(b"@@ -1,5 +1,6 @@"), [1, 5, 1, 6]);
        assert_eq!(
            nums(b"@@ -10 +10 @@"),
            [10, 1, 10, 1],
            "missing count means one line"
        );
        assert_eq!(nums(b"@@ -0,0 +1,3 @@"), [0, 0, 1, 3]);
        assert_eq!(nums(b"@@ -5,3 +5,0 @@"), [5, 3, 5, 0]);
        for bad in [
            &b"@@ -a +1 @@"[..],
            b"@@ -1 +1",
            b"@@ -1,+1 @@",
            b"@@@ -1 -1 +1 @@@",
        ] {
            assert!(
                parse_hunk_header(bad).is_none(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
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
        let s = one(
            b"diff --git a/sp ace b/sp ace\n--- a/sp ace\t\n+++ b/sp ace\t\n@@ -1 +1 @@\n-a\n+b\n",
        );
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
        assert_eq!(
            git_header_paths(b"a/plain \"b/new\\nline\""),
            Some((b"plain".to_vec(), b"new\nline".to_vec()))
        );
    }

    #[test]
    fn unquote_rules() {
        assert_eq!(
            unquote(b"\"a\\\"b\\\\c\\001\"x").unwrap(),
            (b"a\"b\\c\x01".to_vec(), 13)
        );
        assert!(unquote(b"\"unterminated").is_none());
        assert!(unquote(b"\"bad \\q\"").is_none());
        assert!(unquote(b"\"bad \\4xx\"").is_none());
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
}
