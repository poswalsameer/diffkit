//! Local commit history: recent commits on HEAD and the metadata of one
//! commit. Read-only and offline: only the local object database is read.
//!
//! Git subprocesses:
//! - [`resolve_commit`]: `git rev-parse --verify --end-of-options <rev>^{commit}`;
//! - [`recent`] and [`commit`]: one `git log` with an explicit format.
//!
//! `git log` options that keep its output independent of user
//! configuration and keep it from running programs:
//!
//! | Option | Neutralizes |
//! |---|---|
//! | `--no-show-signature` | `log.showSignature` (would run GPG/SSH verification) |
//! | `--no-color` | `color.ui`, `color.log` |
//! | `--no-notes` | `notes.displayRef` and default notes |
//! | `--format=…` with `%aI`/`%cI` | `format.pretty`, `log.date` |
//! | `--no-mailmap` | `log.mailmap` (names are shown as recorded) |
//!
//! Fields are separated by NUL bytes, which commit metadata cannot contain,
//! and each record is validated (object IDs must parse), so a malformed
//! record is reported instead of being misread.

use super::{
    commands::{self, LimitedOutput},
    errors::{GitError, Result},
    models::ObjectId,
};
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    path::Path,
};

/// Default number of commits listed by the history view.
pub const DEFAULT_LIMIT: usize = 200;
/// Upper bound on `git log` output read for one listing.
const MAX_LOG_BYTES: u64 = 32 << 20;

/// Fields per record, in [`FORMAT`] order.
const FIELDS: usize = 10;
const FORMAT: &str = "--format=%H%x00%h%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%B%x00";

/// Who made a commit and when, as recorded in the commit object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Strict ISO 8601 with the recorded time zone, e.g.
    /// `2026-11-06T22:41:08+05:30`.
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: ObjectId,
    /// Git's abbreviation (unique in this repository when listed).
    pub short_id: String,
    /// In order; the first parent is the one a merge is compared against.
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    /// The full message as recorded (not UTF-8 validated by Git; decoded
    /// lossily here). Display code must escape it.
    pub message: String,
    /// This commit's parents are not available because the repository is a
    /// shallow clone and the commit is on its boundary.
    pub shallow_boundary: bool,
}

impl CommitInfo {
    /// The first line of the message.
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("").trim_end()
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    pub fn first_parent(&self) -> Option<&ObjectId> {
        self.parents.first()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// Newest first, following all parents (as `git log` does).
    pub commits: Vec<CommitInfo>,
    /// More commits exist beyond the limit.
    pub truncated: bool,
}

/// Resolves a user-supplied revision (`abc123`, `main`, `HEAD~2`,
/// `v1.0`) to a commit. `--end-of-options` keeps a revision that starts
/// with `-` from being read as an option.
pub fn resolve_commit(root: &Path, rev: &OsStr) -> Result<ObjectId> {
    if rev.is_empty() {
        return Err(GitError::UnknownRevision {
            rev: String::new(),
            detail: "empty revision".into(),
        });
    }
    let mut spec = rev.to_owned();
    spec.push("^{commit}");
    let args: [&OsStr; 5] = [
        OsStr::new("rev-parse"),
        OsStr::new("--verify"),
        OsStr::new("--quiet"),
        OsStr::new("--end-of-options"),
        &spec,
    ];
    let out = commands::output(root, &args)?;
    let line = commands::strip_newline(&out.stdout);
    match (out.status.success(), ObjectId::parse(line)) {
        (true, Some(id)) => Ok(id),
        _ => Err(GitError::UnknownRevision {
            rev: rev.to_string_lossy().into_owned(),
            detail: "no commit with this name exists in the local repository".into(),
        }),
    }
}

/// The most recent commits reachable from HEAD, at most `limit`.
/// `shallow_file` is the repository's `shallow` file (in the common Git
/// directory); commits listed there are marked as shallow boundaries.
pub fn recent(root: &Path, shallow_file: &Path, limit: usize) -> Result<History> {
    let limit = limit.max(1);
    let mut commits = log(root, OsStr::new("HEAD"), limit + 1, shallow_file)?;
    let truncated = commits.len() > limit;
    commits.truncate(limit);
    Ok(History { commits, truncated })
}

/// Metadata of one commit.
pub fn commit(root: &Path, id: &ObjectId, shallow_file: &Path) -> Result<CommitInfo> {
    log(root, OsStr::new(id.as_str()), 1, shallow_file)?
        .pop()
        .ok_or_else(|| GitError::UnknownRevision {
            rev: id.to_string(),
            detail: "the commit is not in the local object database".into(),
        })
}

fn log(root: &Path, rev: &OsStr, count: usize, shallow_file: &Path) -> Result<Vec<CommitInfo>> {
    let args: Vec<OsString> = vec![
        "log".into(),
        "--no-show-signature".into(),
        "--no-color".into(),
        "--no-notes".into(),
        "--no-mailmap".into(),
        FORMAT.into(),
        format!("--max-count={count}").into(),
        "--end-of-options".into(),
        rev.to_owned(),
        "--".into(),
    ];
    let out = match commands::output_limited(root, &args, MAX_LOG_BYTES)? {
        LimitedOutput::Complete(out) if out.status.success() => out,
        LimitedOutput::Complete(out) => return Err(commands::failure(&args, &out)),
        LimitedOutput::Exceeded { limit } => {
            return Err(GitError::malformed(
                "log",
                format!("history output exceeded {limit} bytes"),
            ))
        }
    };
    let shallow = shallow_commits(shallow_file);
    parse(&out.stdout, &shallow)
}

/// Commit IDs on the shallow boundary; empty when the file does not exist
/// (the repository is not shallow).
fn shallow_commits(path: &Path) -> HashSet<String> {
    std::fs::read(path)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Parses records of [`FIELDS`] NUL-terminated fields. `git log` prints a
/// newline between records, so every record after the first starts with one.
pub fn parse(out: &[u8], shallow: &HashSet<String>) -> Result<Vec<CommitInfo>> {
    let mut tokens: Vec<&[u8]> = out.split(|&b| b == 0).collect();
    // What follows the last NUL is only the final newline.
    let tail = tokens.pop().unwrap_or_default();
    if !tail.iter().all(u8::is_ascii_whitespace) {
        return Err(GitError::malformed(
            "log",
            "trailing data after the last record",
        ));
    }
    if !tokens.len().is_multiple_of(FIELDS) {
        return Err(GitError::malformed("log", "incomplete commit record"));
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut commits = Vec::with_capacity(tokens.len() / FIELDS);
    for (i, f) in tokens.chunks(FIELDS).enumerate() {
        let first = if i == 0 {
            f[0]
        } else {
            f[0].strip_prefix(b"\n")
                .ok_or_else(|| GitError::malformed("log", "missing record separator"))?
        };
        let id = ObjectId::parse(first)
            .ok_or_else(|| GitError::malformed("log", format!("bad commit id {}", text(first))))?;
        let parents = if f[2].is_empty() {
            Vec::new()
        } else {
            f[2].split(|&b| b == b' ')
                .map(|p| {
                    ObjectId::parse(p).ok_or_else(|| {
                        GitError::malformed("log", format!("bad parent id {}", text(p)))
                    })
                })
                .collect::<Result<_>>()?
        };
        let shallow_boundary = shallow.contains(id.as_str());
        commits.push(CommitInfo {
            short_id: text(f[1]),
            parents,
            author: Signature {
                name: text(f[3]),
                email: text(f[4]),
                date: text(f[5]),
            },
            committer: Signature {
                name: text(f[6]),
                email: text(f[7]),
                date: text(f[8]),
            },
            message: text(f[9]).trim_end_matches('\n').to_owned(),
            shallow_boundary,
            id,
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";

    fn record(id: &str, parents: &str, msg: &str) -> String {
        format!(
            "{id}\0{}\0{parents}\0Ann\0ann@example.invalid\02026-01-02T03:04:05+00:00\0Cid\0cid@example.invalid\02026-01-03T03:04:05+01:00\0{msg}\0",
            &id[..7]
        )
    }

    #[test]
    fn parses_records_with_multiline_messages_and_merges() {
        let out = format!(
            "{}\n{}\n",
            record(A, &format!("{B} {C}"), "Merge topic\n\nBody line\n"),
            record(B, "", "root\n")
        );
        let shallow = HashSet::from([B.to_owned()]);
        let c = parse(out.as_bytes(), &shallow).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id.as_str(), A);
        assert_eq!(c[0].short_id, "1111111");
        assert!(c[0].is_merge());
        assert_eq!(c[0].first_parent().unwrap().as_str(), B);
        assert_eq!(c[0].subject(), "Merge topic");
        assert_eq!(c[0].message, "Merge topic\n\nBody line");
        assert_eq!(c[0].author.email, "ann@example.invalid");
        assert_eq!(c[0].committer.date, "2026-01-03T03:04:05+01:00");
        assert!(c[1].parents.is_empty() && c[1].shallow_boundary);
        assert!(!c[0].shallow_boundary);
    }

    #[test]
    fn empty_output_is_no_commits() {
        assert!(parse(b"", &HashSet::new()).unwrap().is_empty());
    }

    #[test]
    fn malformed_records_are_errors() {
        let good = record(A, "", "m");
        for bad in [
            good.replacen(A, "nothex", 1),
            good.replacen("\0m\0", "\0m", 1),
            format!("{good}junk"),
            format!("{good}{}", record(B, "zz", "x")),
            format!("{good}{}", record(B, "", "x")),
        ] {
            assert!(parse(bad.as_bytes(), &HashSet::new()).is_err(), "{bad:?}");
        }
    }
}
