//! Structured diff errors. Expected outcomes such as binary, too-large, or
//! unmerged files are [`DiffContent`](super::models::DiffContent) values,
//! not errors.

use crate::git::errors::GitError;
use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum DiffError {
    /// Running Git failed (see [`GitError`]).
    Git(GitError),
    /// `git diff` output did not follow the unified diff format.
    Parse { line: usize, detail: String },
    /// The [`FileId`](super::models::FileId) is not in the list.
    UnknownFile(super::models::FileId),
    /// The file changed after discovery; its metadata no longer describes
    /// what Git reports now. Discover again.
    Stale { path: PathBuf, reason: StaleReason },
    /// An untracked file disappeared after discovery.
    Vanished { path: PathBuf },
    /// A path resolved outside the repository (for example through a
    /// directory replaced by a symlink).
    OutsideRepository { path: PathBuf },
    /// Reading an untracked file failed for a reason other than permissions.
    Io { path: PathBuf, source: io::Error },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// Git no longer reports this path pair as changed.
    MissingFromDiff,
    /// The object ID on a repository side differs from discovery.
    ObjectChanged,
    /// Added/removed line counts differ from discovery.
    StatsChanged,
    /// Text/binary classification differs from discovery.
    BinaryChanged,
    /// Working-tree metadata (size, mtime, type, mode) differs.
    WorkingTreeChanged,
}

impl DiffError {
    pub(crate) fn parse(line: usize, detail: impl Into<String>) -> Self {
        Self::Parse {
            line,
            detail: detail.into(),
        }
    }
}

impl From<GitError> for DiffError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

impl fmt::Display for StaleReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingFromDiff => "Git no longer reports this change",
            Self::ObjectChanged => "an object ID changed",
            Self::StatsChanged => "the added/removed line counts changed",
            Self::BinaryChanged => "the text/binary classification changed",
            Self::WorkingTreeChanged => "the working-tree file changed",
        })
    }
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(e) => e.fmt(f),
            Self::Parse { line, detail } => {
                write!(f, "malformed diff output at line {line}: {detail}")
            }
            Self::UnknownFile(id) => write!(f, "no file with id {} in this diff list", id.0),
            Self::Stale { path, reason } => write!(
                f,
                "{} changed since it was discovered ({reason}); discover again",
                path.display()
            ),
            Self::Vanished { path } => write!(f, "{} no longer exists", path.display()),
            Self::OutsideRepository { path } => {
                write!(f, "{} resolves outside the repository", path.display())
            }
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for DiffError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(e) => Some(e),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, DiffError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_actionable() {
        let e = DiffError::Stale {
            path: "a.txt".into(),
            reason: StaleReason::StatsChanged,
        };
        assert_eq!(
            e.to_string(),
            "a.txt changed since it was discovered (the added/removed line counts changed); discover again"
        );
        assert_eq!(
            DiffError::parse(7, "truncated hunk").to_string(),
            "malformed diff output at line 7: truncated hunk"
        );
    }
}
