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
use crate::{helpers::git_text::unquote, types::repository::ObjectId};

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
