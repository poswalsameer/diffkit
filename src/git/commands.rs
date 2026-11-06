//! The only place where Diffkit starts Git processes.
//!
//! Every invocation runs the `git` executable directly (never a shell),
//! passes arguments as discrete `OsStr` values, pins the target directory
//! with `-C`, and closes stdin. Two environment variables enforce the
//! read-only, offline contract without touching the user's configuration:
//! `GIT_OPTIONAL_LOCKS=0` stops `git status` from taking optional locks and
//! writing opportunistic index refreshes, and `GIT_NO_LAZY_FETCH=1` stops a
//! partial clone from fetching missing objects from its promisor remote
//! (honored by Git 2.45 and later).

use super::errors::{GitError, Result};
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

const GIT: &str = "git";

/// Raw result of a Git process whose exit status the caller interprets.
pub struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs `git -C <dir> <args...>` and returns its output regardless of exit
/// status. Fails only if the process could not be run at all.
pub fn output<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<GitOutput> {
    let out = Command::new(GIT)
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => GitError::GitNotFound,
            io::ErrorKind::PermissionDenied => GitError::GitNotExecutable(e),
            _ => GitError::Io(e),
        })?;
    Ok(GitOutput {
        status: out.status,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Runs Git and returns stdout, turning a non-zero exit into
/// [`GitError::CommandFailed`] with the captured stderr.
pub fn run<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Vec<u8>> {
    let out = output(dir, args)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(failure(args, &out))
    }
}

pub(crate) fn failure<S: AsRef<OsStr>>(args: &[S], out: &GitOutput) -> GitError {
    GitError::CommandFailed {
        args: args
            .iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect(),
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Removes exactly one trailing `\n` (Git's line terminator) without
/// touching other whitespace, which may be part of a path.
pub(crate) fn strip_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

/// Converts a path emitted by Git into a native path without loss.
///
/// On Unix, Git paths are arbitrary bytes and map directly onto `OsStr`.
/// Git for Windows stores and prints paths as UTF-8 and converts them to
/// UTF-16 itself, so on Windows the bytes are decoded strictly as UTF-8;
/// anything else is reported as malformed rather than silently replaced.
#[cfg(unix)]
pub(crate) fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
pub(crate) fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    std::str::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|e| GitError::malformed("path is not valid UTF-8", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_newline_keeps_other_whitespace() {
        assert_eq!(
            strip_newline(b"dir with trailing space \n"),
            b"dir with trailing space "
        );
        assert_eq!(strip_newline(b"a\n\n"), b"a\n");
        assert_eq!(strip_newline(b"a"), b"a");
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_keep_raw_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let p = path_from_git(b"raw-\xff\xfe name\n").unwrap();
        assert_eq!(p.as_os_str().as_bytes(), b"raw-\xff\xfe name\n");
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_decode_utf8_strictly() {
        assert_eq!(
            path_from_git("dir/caf\u{e9} \u{65e5}.txt".as_bytes()).unwrap(),
            PathBuf::from("dir/caf\u{e9} \u{65e5}.txt")
        );
        assert!(path_from_git(b"raw-\xff").is_err());
    }

    #[test]
    fn failing_command_reports_stderr_and_code() {
        let dir = std::env::temp_dir();
        let err = run(
            &dir,
            &["rev-parse", "--verify", "refs/diffkit/definitely-missing"],
        )
        .expect_err("rev-parse of a missing ref must fail");
        match err {
            GitError::CommandFailed { args, code, .. } => {
                assert_eq!(args[0], "rev-parse");
                assert!(code.is_some());
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
