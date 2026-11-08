//! Changed-file discovery from `git diff-index`/`git diff-files` with
//! `--raw --numstat -z`.
//!
//! With `-z`, raw records are `:<old mode> <new mode> <old id> <new id>
//! <status>` followed by one NUL-terminated path (two for renames and
//! copies). Numstat records follow: `<added>\t<removed>\t<path>` or, for
//! renames and copies, `<added>\t<removed>\t` followed by two NUL-terminated
//! paths. Binary files have `-` counts. No file content enters Diffkit's
//! process during discovery.
//!
//! The plumbing commands also list working-tree files whose stat data
//! changed but whose content did not (porcelain `git diff` would refresh the
//! index to hide them, which writes `.git/index`). Such a file has a raw
//! record but no numstat record, because Git compared the content and found
//! no difference. Discovery drops it and counts it in
//! [`DiffFileList::stat_only_entries`](crate::types::diff::DiffFileList).

use crate::{
    diff::{
        errors::{DiffError, Result},
        untracked,
    },
    types::diff::{
        BinaryStatus, DiffComparison, DiffFile, FileChangeKind, FileId, FileMode, LineStats,
    },
};
use crate::{git::paths::path_from_git, types::repository::ObjectId};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecord {
    pub old_mode: u32,
    pub new_mode: u32,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub status: u8,
    pub score: Option<u8>,
    pub path: Vec<u8>,
    /// Destination of a rename or copy (`path` is then the source).
    pub path2: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumstatRecord {
    /// `None` for binary files.
    pub stats: Option<LineStats>,
    pub path: Vec<u8>,
    pub path2: Option<Vec<u8>>,
}

/// A record's path and, for renames and copies, its destination.
type PathPair = (Vec<u8>, Option<Vec<u8>>);

fn malformed(detail: impl Into<String>) -> DiffError {
    DiffError::parse(0, format!("raw/numstat output: {}", detail.into()))
}

pub fn parse_raw_numstat(out: &[u8]) -> Result<Discovered> {
    if out.is_empty() {
        return Ok(Discovered {
            pairs: Vec::new(),
            stat_only: 0,
        });
    }
    let body = out
        .strip_suffix(b"\0")
        .ok_or_else(|| malformed("not NUL-terminated"))?;
    let mut tokens = body.split(|&b| b == 0);
    let mut raws = Vec::new();
    let mut nums = Vec::new();
    while let Some(tok) = tokens.next() {
        let mut next = || {
            tokens
                .next()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| malformed("missing path"))
        };
        if let Some(meta) = tok.strip_prefix(b":") {
            let mut r = raw_meta(meta)?;
            r.path = next()?.to_vec();
            if matches!(r.status, b'R' | b'C') {
                r.path2 = Some(next()?.to_vec());
            }
            raws.push(r);
        } else {
            let mut parts = tok.splitn(3, |&b| b == b'\t');
            let (a, d, rest) = match (parts.next(), parts.next(), parts.next()) {
                (Some(a), Some(d), Some(rest)) => (a, d, rest),
                _ => return Err(malformed("bad numstat record")),
            };
            let stats = match (a, d) {
                (b"-", b"-") => None,
                _ => Some(LineStats {
                    added: count(a)?,
                    removed: count(d)?,
                }),
            };
            let (path, path2) = if rest.is_empty() {
                (next()?.to_vec(), Some(next()?.to_vec()))
            } else {
                (rest.to_vec(), None)
            };
            nums.push(NumstatRecord { stats, path, path2 });
        }
    }
    // Pair each raw record with the numstat record for the same paths, in
    // order (an unmerged path has two of each).
    let mut by_key: HashMap<PathPair, VecDeque<NumstatRecord>> = HashMap::new();
    for n in nums {
        by_key
            .entry((n.path.clone(), n.path2.clone()))
            .or_default()
            .push_back(n);
    }
    let mut pairs = Vec::with_capacity(raws.len());
    let mut stat_only = 0;
    for r in raws {
        match by_key
            .get_mut(&(r.path.clone(), r.path2.clone()))
            .and_then(VecDeque::pop_front)
        {
            Some(n) => pairs.push((r, n)),
            None => stat_only += 1,
        }
    }
    if by_key.values().any(|q| !q.is_empty()) {
        return Err(malformed("numstat record without a raw record"));
    }
    Ok(Discovered { pairs, stat_only })
}

/// Raw records paired with their numstat records.
pub struct Discovered {
    pub pairs: Vec<(RawRecord, NumstatRecord)>,
    /// Raw records without a numstat record: content unchanged.
    pub stat_only: usize,
}

fn count(b: &[u8]) -> Result<u64> {
    std::str::from_utf8(b)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| malformed(format!("bad count {:?}", String::from_utf8_lossy(b))))
}

fn raw_meta(meta: &[u8]) -> Result<RawRecord> {
    let f: Vec<&[u8]> = meta.split(|&b| b == b' ').collect();
    if f.len() != 5 {
        return Err(malformed("raw record must have 5 fields"));
    }
    let mode = |b: &[u8]| {
        std::str::from_utf8(b)
            .ok()
            .and_then(|s| u32::from_str_radix(s, 8).ok())
            .ok_or_else(|| malformed("bad mode"))
    };
    let oid = |b: &[u8]| {
        ObjectId::parse(b)
            .ok_or_else(|| malformed("bad object id"))
            .map(|o| (!o.is_zero()).then_some(o))
    };
    let (&status, score) = f[4]
        .split_first()
        .ok_or_else(|| malformed("empty status"))?;
    let score = if score.is_empty() {
        None
    } else {
        Some(count(score)? as u8)
    };
    Ok(RawRecord {
        old_mode: mode(f[0])?,
        new_mode: mode(f[1])?,
        old_oid: oid(f[2])?,
        new_oid: oid(f[3])?,
        status,
        score,
        path: Vec::new(),
        path2: None,
    })
}

/// Builds the file list. An unmerged path appears in `git diff` (unstaged)
/// as a `U` record plus a record against "ours"; the extra records are
/// folded into the single `Unmerged` entry.
pub fn build_files(
    records: Vec<(RawRecord, NumstatRecord)>,
    comparison: DiffComparison,
    root: &Path,
    conflicted: impl Fn(&Path) -> bool,
) -> Result<Vec<DiffFile>> {
    let unmerged: HashSet<Vec<u8>> = records
        .iter()
        .filter(|(r, _)| r.status == b'U')
        .map(|(r, _)| r.path.clone())
        .collect();
    let mut files = Vec::with_capacity(records.len());
    for (r, n) in records {
        if r.status != b'U' && r.path2.is_none() && unmerged.contains(&r.path) {
            continue;
        }
        let path = path_from_git(&r.path)?;
        let path2 = r.path2.as_deref().map(path_from_git).transpose()?;
        let mode = |m: u32| (m != 0).then_some(FileMode(m));
        let (old_mode, new_mode) = (mode(r.old_mode), mode(r.new_mode));
        let mut kind = match r.status {
            b'A' => FileChangeKind::Added,
            b'D' => FileChangeKind::Deleted,
            b'M' => FileChangeKind::Modified,
            b'R' => FileChangeKind::Renamed,
            b'C' => FileChangeKind::Copied,
            b'T' => FileChangeKind::TypeChanged,
            b'U' => FileChangeKind::Unmerged,
            other => FileChangeKind::Unknown(other),
        };
        let same_content = match (&r.old_oid, &r.new_oid) {
            (Some(a), Some(b)) => a == b,
            _ => n.stats == Some(LineStats::default()),
        };
        if kind == FileChangeKind::Modified && old_mode != new_mode && same_content {
            kind = FileChangeKind::ModeChanged;
        }
        let (old_path, new_path) = match kind {
            FileChangeKind::Added => (None, Some(path)),
            FileChangeKind::Deleted => (Some(path), None),
            FileChangeKind::Renamed | FileChangeKind::Copied => (Some(path), path2),
            _ => (Some(path.clone()), Some(path)),
        };
        let unmerged_entry = kind == FileChangeKind::Unmerged;
        let worktree_stamp = match (&new_path, comparison.new_side_is_working_tree()) {
            (Some(p), true) => untracked::stamp(&root.join(p)).ok(),
            _ => None,
        };
        let is_conflicted = unmerged_entry || new_path.as_deref().is_some_and(&conflicted);
        files.push(DiffFile {
            id: FileId(files.len()),
            comparison,
            old_path,
            new_path,
            kind,
            old_mode,
            new_mode,
            similarity: r.score.filter(|_| matches!(r.status, b'R' | b'C')),
            old_oid: r.old_oid,
            new_oid: r.new_oid,
            binary: match (unmerged_entry, n.stats) {
                (true, _) => BinaryStatus::Unknown,
                (false, None) => BinaryStatus::Binary,
                (false, Some(_)) => BinaryStatus::Text,
            },
            stats: if unmerged_entry { None } else { n.stats },
            conflicted: is_conflicted,
            worktree_stamp,
        });
    }
    Ok(files)
}
