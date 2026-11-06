//! Structured errors for the Git layer.
//!
//! Expected repository conditions (no upstream, detached HEAD, unborn
//! branch, empty repository) are *not* errors; they are represented in the
//! models. Only environmental, execution, and parsing failures live here.

use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum GitError {
    /// The `git` executable could not be found on `PATH`.
    GitNotFound,
    /// The `git` executable exists but could not be started.
    GitNotExecutable(io::Error),
    /// Any other failure while spawning or waiting for Git.
    Io(io::Error),
    /// The directory to inspect does not exist or cannot be accessed.
    PathInaccessible { path: PathBuf, source: io::Error },
    /// The directory is not inside any Git repository.
    NotARepository { path: PathBuf, stderr: String },
    /// The directory belongs to a repository but not to a working tree
    /// (a bare repository or the inside of a Git directory).
    NotAWorkTree { path: PathBuf },
    /// Git ran but exited unsuccessfully.
    CommandFailed {
        args: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
    /// Git produced output that does not follow the documented format.
    MalformedOutput {
        context: &'static str,
        detail: String,
    },
}

impl GitError {
    pub(crate) fn malformed(context: &'static str, detail: impl Into<String>) -> Self {
        Self::MalformedOutput {
            context,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitNotFound => write!(
                f,
                "the `git` executable was not found; install Git and make sure it is on PATH"
            ),
            Self::GitNotExecutable(e) => write!(f, "the `git` executable could not be run: {e}"),
            Self::Io(e) => write!(f, "could not run Git: {e}"),
            Self::PathInaccessible { path, source } => {
                write!(f, "cannot access {}: {source}", path.display())
            }
            Self::NotARepository { path, stderr } => {
                write!(f, "{} is not inside a Git repository", path.display())?;
                if !stderr.trim().is_empty() {
                    write!(f, " (git: {})", stderr.trim())?;
                }
                Ok(())
            }
            Self::NotAWorkTree { path } => write!(
                f,
                "{} is inside a Git directory or bare repository, not a working tree",
                path.display()
            ),
            Self::CommandFailed { args, code, stderr } => {
                write!(f, "`git {}` failed", args.join(" "))?;
                match code {
                    Some(c) => write!(f, " with exit code {c}")?,
                    None => write!(f, " (terminated by a signal)")?,
                }
                if !stderr.trim().is_empty() {
                    write!(f, ": {}", stderr.trim())?;
                }
                Ok(())
            }
            Self::MalformedOutput { context, detail } => {
                write!(f, "unexpected Git output ({context}): {detail}")
            }
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GitNotExecutable(e) | Self::Io(e) => Some(e),
            Self::PathInaccessible { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_failure_message_includes_args_code_and_stderr() {
        let e = GitError::CommandFailed {
            args: vec!["status".into(), "-z".into()],
            code: Some(128),
            stderr: "fatal: index file corrupt\n".into(),
        };
        assert_eq!(
            e.to_string(),
            "`git status -z` failed with exit code 128: fatal: index file corrupt"
        );
    }

    #[test]
    fn not_a_repository_message_names_path() {
        let e = GitError::NotARepository {
            path: PathBuf::from("/tmp/x"),
            stderr: String::new(),
        };
        assert_eq!(e.to_string(), "/tmp/x is not inside a Git repository");
    }

    #[test]
    fn git_not_found_is_actionable() {
        assert!(GitError::GitNotFound.to_string().contains("PATH"));
    }
}
