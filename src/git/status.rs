//! Reads and parses `git status --porcelain=v2 -z --branch`.
//!
//! The output is a sequence of NUL-terminated records. Paths may contain
//! any byte except NUL (spaces, tabs, newlines, non-UTF-8), so the parser
//! works on bytes and never splits on newlines. Rename/copy records are
//! followed by an extra NUL-terminated field holding the original path.

use super::{
    commands,
    errors::{GitError, Result},
    models::{
        Change, ConflictKind, FileKind, FileStatus, RepositoryStatus, Similarity, SubmoduleState,
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

pub fn read(root: &Path) -> Result<StatusReport> {
    parse(&commands::run(root, &STATUS_ARGS)?)
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
        path: commands::path_from_git(f[7])?,
        original_path: None,
        index: change(xy[0]),
        worktree: change(xy[1]),
        xy,
        conflict: None,
        similarity: None,
        file_kind: file_kind(&modes),
        submodule: submodule(f[1], CTX)?,
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
        path: commands::path_from_git(f[8])?,
        original_path: Some(commands::path_from_git(original)?),
        index: change(xy[0]),
        worktree: change(xy[1]),
        xy,
        conflict: None,
        similarity: Some(Similarity(score)),
        file_kind: file_kind(&modes),
        submodule: submodule(f[1], CTX)?,
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
    Ok(FileStatus {
        path: commands::path_from_git(f[9])?,
        original_path: None,
        index: Change::Unmerged,
        worktree: Change::Unmerged,
        xy,
        conflict: Some(conflict),
        similarity: None,
        file_kind: file_kind(&modes),
        submodule: submodule(f[1], CTX)?,
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
    })
}

fn non_empty_path(bytes: &[u8], context: &'static str) -> Result<std::path::PathBuf> {
    if bytes.is_empty() {
        return Err(GitError::malformed(context, "empty path"));
    }
    commands::path_from_git(bytes)
}

fn show(bytes: &[u8]) -> String {
    format!("{:?}", String::from_utf8_lossy(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const H: &str = "100644 100644 100644 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

    fn one(s: &str) -> FileStatus {
        let mut files = parse(s.as_bytes()).unwrap().status.files;
        assert_eq!(files.len(), 1);
        files.remove(0)
    }

    #[test]
    fn empty_output_is_clean() {
        let r = parse(b"").unwrap();
        assert!(r.status.is_clean());
        assert_eq!(r.headers, BranchHeaders::default());
    }

    #[test]
    fn branch_headers() {
        let r = parse(
            b"# branch.oid 1111111111111111111111111111111111111111\0# branch.head feature/x y\0\
              # branch.upstream origin/feature/x\0# branch.ab +3 -2\0# stash 4\0",
        )
        .unwrap();
        assert_eq!(r.headers.head.as_deref(), Some(&b"feature/x y"[..]));
        assert_eq!(
            r.headers.upstream.as_deref(),
            Some(&b"origin/feature/x"[..])
        );
        assert_eq!(r.headers.ab.as_deref(), Some(&b"+3 -2"[..]));
        assert!(r.status.is_clean());
    }

    #[test]
    fn ordinary_record_preserves_both_dimensions_and_spaces() {
        let f = one(&format!("1 MM N... {H} dir/a file  with spaces\0"));
        assert_eq!(f.path, PathBuf::from("dir/a file  with spaces"));
        assert_eq!((f.index, f.worktree), (Change::Modified, Change::Modified));
        assert_eq!(&f.xy, b"MM");
        assert_eq!(f.file_kind, Some(FileKind::Regular));
        assert!(f.is_partially_staged());
    }

    #[test]
    fn deleted_file_kind_falls_back_to_head_mode() {
        let f = one("1 D. N... 120000 000000 000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0000000000000000000000000000000000000000 link\0");
        assert_eq!(f.index, Change::Deleted);
        assert_eq!(f.file_kind, Some(FileKind::Symlink));
    }

    #[test]
    fn rename_record_consumes_original_path_field() {
        let r =
            parse(format!("2 R. N... {H} R87 new name\0old\nname\0? after\0").as_bytes()).unwrap();
        let f = &r.status.files[0];
        assert_eq!(f.path, PathBuf::from("new name"));
        assert_eq!(f.original_path, Some(PathBuf::from("old\nname")));
        assert_eq!(f.similarity, Some(Similarity(87)));
        assert_eq!(f.index, Change::Renamed);
        assert_eq!(r.status.files[1].path, PathBuf::from("after"));
        assert!(r.status.files[1].is_untracked());
    }

    #[test]
    fn copy_record() {
        let r = parse(format!("2 C. N... {H} C100 copy\0src\0").as_bytes()).unwrap();
        let f = &r.status.files[0];
        assert!(f.is_copied() && f.is_staged() && !f.is_renamed());
        assert_eq!(f.original_path, Some(PathBuf::from("src")));
    }

    #[test]
    fn unmerged_record_keeps_conflict_kind() {
        let f = one("u AU N... 000000 100644 000000 100644 0000000000000000000000000000000000000000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0000000000000000000000000000000000000000 c d\0");
        assert_eq!(f.conflict, Some(ConflictKind::AddedByUs));
        assert_eq!(f.path, PathBuf::from("c d"));
        assert_eq!(&f.xy, b"AU");
        assert!(f.is_conflicted() && !f.is_staged() && !f.is_unstaged());
    }

    #[test]
    fn untracked_and_ignored_are_kept_apart() {
        let r = parse(b"? new\tfile\0! target/out\0").unwrap();
        assert_eq!(r.status.files.len(), 1);
        assert!(r.status.files[0].is_untracked());
        assert_eq!(r.status.files[0].path, PathBuf::from("new\tfile"));
        assert_eq!(r.status.ignored, vec![PathBuf::from("target/out")]);
        assert_eq!(r.status.untracked_count(), 1);
    }

    #[test]
    fn submodule_field() {
        let f = one("1 .M SCMU 160000 160000 160000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 sub\0");
        assert_eq!(f.file_kind, Some(FileKind::Submodule));
        assert_eq!(
            f.submodule,
            Some(SubmoduleState {
                commit_changed: true,
                has_tracked_changes: true,
                has_untracked_changes: true
            })
        );
    }

    #[test]
    fn unknown_status_letter_is_preserved_not_rejected() {
        let f = one(&format!("1 .X N... {H} f\0"));
        assert_eq!(f.worktree, Change::Unknown(b'X'));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_bytes_are_preserved() {
        use std::os::unix::ffi::OsStrExt;
        let mut raw = format!("1 .M N... {H} raw-").into_bytes();
        raw.extend_from_slice(b"\xff\xfe\0? \xc3\x28\0");
        let r = parse(&raw).unwrap();
        assert_eq!(
            r.status.files[0].path.as_os_str().as_bytes(),
            b"raw-\xff\xfe"
        );
        assert_eq!(r.status.files[1].path.as_os_str().as_bytes(), b"\xc3\x28");
    }

    #[test]
    fn malformed_inputs_are_errors_not_panics() {
        let bad: &[&[u8]] = &[
            b"1 M. N...\0",
            b"1 M N... 100644 100644 100644 a b f\0",
            b"1 M. N... 10064x 100644 100644 a b f\0",
            b"2 R. N... 100644 100644 100644 a b R50 new\0",
            b"2 R. N... 100644 100644 100644 a b Q50 new\0old\0",
            b"u ZZ N... 100644 100644 100644 100644 a b c f\0",
            b"1 M. Nxyz 100644 100644 100644 a b f\0",
            b"? \0",
            b"Z what\0",
            b"\0",
            b"#\0",
            b"? unterminated",
        ];
        for input in bad {
            assert!(
                parse(input).is_err(),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }
}
