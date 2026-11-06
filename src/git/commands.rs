//! The only place where Diffkit starts Git processes.
//!
//! Every invocation runs the `git` executable directly (never a shell),
//! passes arguments as discrete `OsStr` values, pins the target directory
//! with `-C`, and closes stdin. Environment variables enforce the
//! read-only, offline contract without touching the user's configuration:
//!
//! - `GIT_OPTIONAL_LOCKS=0` stops `git status`/`git diff` from taking
//!   optional locks and writing opportunistic index refreshes.
//! - `GIT_NO_LAZY_FETCH=1` stops a partial clone from fetching missing
//!   objects from its promisor remote (honored by Git 2.45 and later).
//! - `GIT_LITERAL_PATHSPECS=1` (with the glob/icase variants removed) makes
//!   every pathspec a literal path, so filenames are never patterns.
//! - `GIT_DIFF_OPTS` is removed so it cannot override the context size
//!   Diffkit requests explicitly.

use super::errors::{GitError, Result};
use std::{
    borrow::Cow,
    cell::Cell,
    ffi::OsStr,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

const GIT: &str = "git";

thread_local! {
    static INVOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// Number of Git processes started by the current thread. Used by tests and
/// benchmarks to verify that no redundant subprocesses are spawned.
pub fn thread_invocations() -> u64 {
    INVOCATIONS.with(Cell::get)
}

/// Raw result of a Git process whose exit status the caller interprets.
pub struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Result of [`output_limited`].
pub enum LimitedOutput {
    Complete(GitOutput),
    /// Standard output exceeded the limit; the process was killed and the
    /// partial output discarded.
    Exceeded {
        limit: u64,
    },
}

fn command<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Command {
    INVOCATIONS.with(|n| n.set(n.get() + 1));
    let mut c = Command::new(GIT);
    c.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env_remove("GIT_GLOB_PATHSPECS")
        .env_remove("GIT_NOGLOB_PATHSPECS")
        .env_remove("GIT_ICASE_PATHSPECS")
        .env_remove("GIT_DIFF_OPTS")
        .stdin(Stdio::null());
    c
}

fn spawn_error(e: io::Error) -> GitError {
    match e.kind() {
        io::ErrorKind::NotFound => GitError::GitNotFound,
        io::ErrorKind::PermissionDenied => GitError::GitNotExecutable(e),
        _ => GitError::Io(e),
    }
}

/// Runs `git -C <dir> <args...>` and returns its output regardless of exit
/// status. Fails only if the process could not be run at all.
pub fn output<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<GitOutput> {
    let out = command(dir, args).output().map_err(spawn_error)?;
    Ok(GitOutput {
        status: out.status,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Like [`output`], but stops reading once stdout exceeds `limit` bytes,
/// kills Git, and reports [`LimitedOutput::Exceeded`] instead of returning
/// a truncated result. Stderr is drained on a scoped thread that is joined
/// before returning, so a chatty stderr cannot block the process.
pub fn output_limited<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    limit: u64,
) -> Result<LimitedOutput> {
    let mut child = command(dir, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_error)?;
    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let (read, stderr) = std::thread::scope(|scope| {
        let errs = scope.spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf);
            buf
        });
        let mut stdout = Vec::new();
        // Read one byte past the limit to detect overflow.
        let read = (&mut stdout_pipe)
            .take(limit.saturating_add(1))
            .read_to_end(&mut stdout)
            .map(|_| stdout);
        let exceeded = matches!(&read, Ok(b) if b.len() as u64 > limit);
        if exceeded {
            let _ = child.kill();
        }
        drop(stdout_pipe);
        (read, errs.join().unwrap_or_default())
    });
    let status = child.wait().map_err(GitError::Io)?;
    let stdout = read.map_err(GitError::Io)?;
    if stdout.len() as u64 > limit {
        return Ok(LimitedOutput::Exceeded { limit });
    }
    Ok(LimitedOutput::Complete(GitOutput {
        status,
        stdout,
        stderr,
    }))
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
pub fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
pub fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    std::str::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|e| GitError::malformed("path is not valid UTF-8", e.to_string()))
}

/// The bytes Git uses for a native path: the inverse of [`path_from_git`].
#[cfg(unix)]
pub fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(path.as_os_str().as_bytes())
}

/// The bytes Git uses for a native path: the inverse of [`path_from_git`].
/// Paths produced by Diffkit are valid UTF-8 on Windows; any other path is
/// converted with replacement and can never match a Git path.
#[cfg(not(unix))]
pub fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    match path.to_string_lossy() {
        Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
        Cow::Owned(s) => Cow::Owned(s.into_bytes()),
    }
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

    #[test]
    fn limited_output_reports_overflow_instead_of_truncating() {
        let dir = std::env::temp_dir();
        match output_limited(&dir, &["--version"], 4).unwrap() {
            LimitedOutput::Exceeded { limit } => assert_eq!(limit, 4),
            LimitedOutput::Complete(_) => panic!("`git --version` is longer than 4 bytes"),
        }
        match output_limited(&dir, &["--version"], 1 << 20).unwrap() {
            LimitedOutput::Complete(out) => {
                assert!(out.status.success());
                assert!(out.stdout.starts_with(b"git version"));
            }
            LimitedOutput::Exceeded { .. } => panic!("unexpected overflow"),
        }
    }

    #[test]
    fn invocations_are_counted_per_thread() {
        let before = thread_invocations();
        let _ = output(&std::env::temp_dir(), &["--version"]);
        assert_eq!(thread_invocations(), before + 1);
    }
}
