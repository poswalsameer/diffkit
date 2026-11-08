//! Reads and parses `git status --porcelain=v2 -z --branch`.
//!
//! The output is a sequence of NUL-terminated records. Paths may contain
//! any byte except NUL (spaces, tabs, newlines, non-UTF-8), so the parser
//! works on bytes and never splits on newlines. Rename/copy records are
//! followed by an extra NUL-terminated field holding the original path.

use crate::{
    git::{
        commands,
        errors::{GitError, Result},
        paths,
    },
    types::repository::{
        Change, ConflictKind, ConflictStage, ConflictStages, FileKind, FileStatus, ObjectId,
        RepositoryStatus, Similarity, SubmoduleState,
    },
};
use std::path::Path;

/// The `# branch.*` headers, kept raw so the branch and commit layers can
/// interpret them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchHeaders {
    pub oid: Option<Vec<u8>>,
    pub head: Option<Vec<u8>>,
    pub upstream: Option<Vec<u8>>,
    pub ab: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusReport {
    pub headers: BranchHeaders,
    pub status: RepositoryStatus,
}

/// The single status invocation used by inspection. `--ahead-behind` is
/// explicit so a user's `status.aheadBehind=false` cannot hide the counts.
pub const STATUS_ARGS: [&str; 6] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--branch",
    "--ahead-behind",
    "--untracked-files=all",
];

/// Like [`STATUS_ARGS`], without the ahead/behind walk (`branch.ab` is then
/// `+? -?` unless the two commits are equal). Used when the counts from a
/// previous snapshot are known to still hold.
pub const STATUS_ARGS_NO_AHEAD_BEHIND: [&str; 6] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--branch",
    "--no-ahead-behind",
    "--untracked-files=all",
];

pub fn read(root: &Path) -> Result<StatusReport> {
    parse(&commands::run(root, &STATUS_ARGS)?)
}

/// [`read`] without computing ahead/behind counts.
pub fn read_without_ahead_behind(root: &Path) -> Result<StatusReport> {
    parse(&commands::run(root, &STATUS_ARGS_NO_AHEAD_BEHIND)?)
}

pub fn parse(raw: &[u8]) -> Result<StatusReport> {
    let mut report = StatusReport::default();
    let body = match raw {
        [] => return Ok(report),
        [rest @ .., 0] => rest,
        _ => {
            return Err(GitError::malformed(
                "status",
                "output is not NUL-terminated",
            ))
        }
    };
    let mut records = body.split(|&b| b == 0);
    while let Some(rec) = records.next() {
        let (&kind, rest) = rec
            .split_first()
            .ok_or_else(|| GitError::malformed("status", "empty record"))?;
        let rest = rest
            .strip_prefix(b" ")
            .ok_or_else(|| GitError::malformed("status", format!("record {}", show(rec))))?;
        match kind {
            b'#' => header(&mut report.headers, rest),
            b'1' => report.status.files.push(ordinary(rest)?),
            b'2' => {
                let original = records.next().ok_or_else(|| {
                    GitError::malformed("rename/copy record", "missing original path")
                })?;
                report.status.files.push(rename_or_copy(rest, original)?);
            }
            b'u' => report.status.files.push(unmerged(rest)?),
            b'?' => report.status.files.push(untracked(rest)?),
            b'!' => report
                .status
                .ignored
                .push(non_empty_path(rest, "ignored record")?),
            _ => {
                return Err(GitError::malformed(
                    "status",
                    format!("unknown record type in {}", show(rec)),
                ))
            }
        }
    }
    Ok(report)
}

fn header(h: &mut BranchHeaders, rest: &[u8]) {
    let (key, value) = match rest.iter().position(|&b| b == b' ') {
        Some(i) => (&rest[..i], rest[i + 1..].to_vec()),
        None => (rest, Vec::new()),
    };
    // Other headers (e.g. `# stash`) are not needed and are ignored.
    match key {
        b"branch.oid" => h.oid = Some(value),
        b"branch.head" => h.head = Some(value),
        b"branch.upstream" => h.upstream = Some(value),
        b"branch.ab" => h.ab = Some(value),
        _ => {}
    }
}

/// Splits `n` space-separated fields; the last one is the path and keeps
/// any spaces it contains.
fn fields<'a>(rest: &'a [u8], n: usize, context: &'static str) -> Result<Vec<&'a [u8]>> {
    let f: Vec<&[u8]> = rest.splitn(n, |&b| b == b' ').collect();
    if f.len() != n || f[n - 1].is_empty() {
        return Err(GitError::malformed(context, show(rest)));
    }
    Ok(f)
}

fn xy(field: &[u8], context: &'static str) -> Result<[u8; 2]> {
    <[u8; 2]>::try_from(field).map_err(|_| GitError::malformed(context, show(field)))
}

fn change(b: u8) -> Change {
    match b {
        b'.' => Change::Unmodified,
        b'M' => Change::Modified,
        b'T' => Change::TypeChanged,
        b'A' => Change::Added,
        b'D' => Change::Deleted,
        b'R' => Change::Renamed,
        b'C' => Change::Copied,
        b'U' => Change::Unmerged,
        other => Change::Unknown(other),
    }
}

fn mode(field: &[u8], context: &'static str) -> Result<u32> {
    std::str::from_utf8(field)
        .ok()
        .filter(|s| s.len() == 6)
        .and_then(|s| u32::from_str_radix(s, 8).ok())
        .ok_or_else(|| GitError::malformed(context, format!("mode {}", show(field))))
}

/// Uses the worktree mode, falling back to index then HEAD for deletions.
fn file_kind(modes: &[u32]) -> Option<FileKind> {
    modes.iter().rev().find_map(|&m| FileKind::from_mode(m))
}

fn submodule(field: &[u8], context: &'static str) -> Result<Option<SubmoduleState>> {
    match field {
        b"N..." => Ok(None),
        [b'S', c, m, u] if [c, m, u].iter().all(|b| b"CMU.".contains(b)) => {
            Ok(Some(SubmoduleState {
                commit_changed: *c == b'C',
                has_tracked_changes: *m == b'M',
                has_untracked_changes: *u == b'U',
            }))
        }
        _ => Err(GitError::malformed(
            context,
            format!("submodule field {}", show(field)),
        )),
    }
}

// 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
fn ordinary(rest: &[u8]) -> Result<FileStatus> {
    const CTX: &str = "ordinary record";
    let f = fields(rest, 8, CTX)?;
    let xy = xy(f[0], CTX)?;
    let modes = [mode(f[2], CTX)?, mode(f[3], CTX)?, mode(f[4], CTX)?];
    Ok(FileStatus {
        path: paths::path_from_git(f[7])?,
        original_path: None,
        index: change(xy[0]),
        worktree: change(xy[1]),
        xy,
        conflict: None,
        similarity: None,
        file_kind: file_kind(&modes),
        submodule: submodule(f[1], CTX)?,
        stages: None,
    })
}

// 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>
fn rename_or_copy(rest: &[u8], original: &[u8]) -> Result<FileStatus> {
    const CTX: &str = "rename/copy record";
    let f = fields(rest, 9, CTX)?;
    let xy = xy(f[0], CTX)?;
    let modes = [mode(f[2], CTX)?, mode(f[3], CTX)?, mode(f[4], CTX)?];
    let score = match f[7] {
        [b'R' | b'C', digits @ ..] => std::str::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse().ok()),
        _ => None,
    }
    .ok_or_else(|| GitError::malformed(CTX, format!("score {}", show(f[7]))))?;
    if original.is_empty() {
        return Err(GitError::malformed(CTX, "empty original path"));
    }
    Ok(FileStatus {
        path: paths::path_from_git(f[8])?,
        original_path: Some(paths::path_from_git(original)?),
        index: change(xy[0]),
        worktree: change(xy[1]),
        xy,
        conflict: None,
        similarity: Some(Similarity(score)),
        file_kind: file_kind(&modes),
        submodule: submodule(f[1], CTX)?,
        stages: None,
    })
}

// u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
fn unmerged(rest: &[u8]) -> Result<FileStatus> {
    const CTX: &str = "unmerged record";
    let f = fields(rest, 10, CTX)?;
    let xy = xy(f[0], CTX)?;
    let conflict = ConflictKind::from_xy(xy)
        .ok_or_else(|| GitError::malformed(CTX, format!("conflict code {}", show(&xy))))?;
    let modes = [
        mode(f[2], CTX)?,
        mode(f[3], CTX)?,
        mode(f[4], CTX)?,
        mode(f[5], CTX)?,
    ];
    // A stage that does not exist has mode 000000 and the zero object ID.
    let stage = |m: u32, h: &[u8]| -> Result<Option<ConflictStage>> {
        let oid = ObjectId::parse(h)
            .ok_or_else(|| GitError::malformed(CTX, format!("object id {}", show(h))))?;
        Ok((m != 0 && !oid.is_zero()).then_some(ConflictStage { mode: m, oid }))
    };
    let stages = ConflictStages {
        base: stage(modes[0], f[6])?,
        ours: stage(modes[1], f[7])?,
        theirs: stage(modes[2], f[8])?,
    };
    Ok(FileStatus {
        path: paths::path_from_git(f[9])?,
        original_path: None,
        index: Change::Unmerged,
        worktree: Change::Unmerged,
        xy,
        conflict: Some(conflict),
        similarity: None,
        file_kind: file_kind(&modes),
        submodule: submodule(f[1], CTX)?,
        stages: Some(stages),
    })
}

fn untracked(rest: &[u8]) -> Result<FileStatus> {
    Ok(FileStatus {
        path: non_empty_path(rest, "untracked record")?,
        original_path: None,
        index: Change::Untracked,
        worktree: Change::Untracked,
        xy: *b"??",
        conflict: None,
        similarity: None,
        file_kind: None,
        submodule: None,
        stages: None,
    })
}

fn non_empty_path(bytes: &[u8], context: &'static str) -> Result<std::path::PathBuf> {
    if bytes.is_empty() {
        return Err(GitError::malformed(context, "empty path"));
    }
    paths::path_from_git(bytes)
}

fn show(bytes: &[u8]) -> String {
    format!("{:?}", String::from_utf8_lossy(bytes))
}
