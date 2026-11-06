//! Repository discovery: root, Git directory, common directory, worktree
//! kind, and shallowness, all resolved by Git itself.

use super::{
    commands,
    errors::{GitError, Result},
    models::{RepositoryInfo, RepositoryKind},
};
use std::path::{Path, PathBuf};

/// Flags whose answers are `true`/`false`, then the three path queries.
/// `--show-toplevel` is last: outside a working tree it is the one that
/// fails, and the answers printed before it are still readable.
const FLAGS: [&str; 3] = [
    "--is-inside-work-tree",
    "--is-bare-repository",
    "--is-shallow-repository",
];
const PATHS: [&str; 3] = ["--absolute-git-dir", "--git-common-dir", "--show-toplevel"];

pub fn discover() -> Result<RepositoryInfo> {
    discover_from(&current_dir()?)
}

pub(crate) fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().map_err(|source| GitError::PathInaccessible {
        path: PathBuf::from("."),
        source,
    })
}

/// Resolves the starting directory the way Git reports paths: with
/// symlinks resolved on Unix (Git prints real paths there), and as a plain
/// absolute path on Windows, where `canonicalize` would add a `\\?\`
/// prefix that Git never prints.
fn resolve_start(start: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    {
        std::fs::metadata(start)?;
        std::path::absolute(start)
    }
    #[cfg(not(windows))]
    {
        start.canonicalize()
    }
}

pub fn discover_from(start: &Path) -> Result<RepositoryInfo> {
    let cwd = resolve_start(start).map_err(|source| GitError::PathInaccessible {
        path: start.to_path_buf(),
        source,
    })?;
    let mut args = vec!["rev-parse", "--path-format=absolute"];
    args.extend(FLAGS);
    args.extend(PATHS);
    let out = commands::output(&cwd, &args)?;
    let lines: Vec<&[u8]> = commands::strip_newline(&out.stdout)
        .split(|&b| b == b'\n')
        .collect();
    if !out.status.success() {
        return Err(classify_failure(&cwd, &args, &out, lines.first().copied()));
    }
    let flag = |i: usize| -> Result<bool> {
        match lines.get(i).copied() {
            Some(b"true") => Ok(true),
            Some(b"false") => Ok(false),
            other => Err(GitError::malformed(
                "rev-parse",
                format!("expected true/false for {}, got {other:?}", FLAGS[i]),
            )),
        }
    };
    let (inside, bare, shallow) = (flag(0)?, flag(1)?, flag(2)?);
    if !inside || bare {
        return Err(GitError::NotAWorkTree { path: cwd });
    }
    // rev-parse cannot NUL-terminate, so a path containing a newline makes
    // the combined answer ambiguous. Fall back to one query per path.
    let paths = if lines.len() == FLAGS.len() + PATHS.len() {
        lines[FLAGS.len()..]
            .iter()
            .map(|l| absolute(&cwd, l))
            .collect::<Result<Vec<_>>>()?
    } else {
        PATHS
            .iter()
            .map(|p| {
                let out = commands::run(&cwd, &["rev-parse", "--path-format=absolute", p])?;
                absolute(&cwd, commands::strip_newline(&out))
            })
            .collect::<Result<Vec<_>>>()?
    };
    let [git_dir, common_git_dir, root]: [PathBuf; 3] = paths
        .try_into()
        .map_err(|_| GitError::malformed("rev-parse", "missing repository paths"))?;
    let kind = if git_dir == common_git_dir {
        RepositoryKind::Standard
    } else {
        RepositoryKind::LinkedWorktree
    };
    Ok(RepositoryInfo {
        root,
        git_dir,
        common_git_dir,
        current_directory: cwd,
        kind,
        is_inside_work_tree: inside,
        shallow,
    })
}

fn absolute(cwd: &Path, bytes: &[u8]) -> Result<PathBuf> {
    if bytes.is_empty() {
        return Err(GitError::malformed("rev-parse", "empty path"));
    }
    let p = commands::path_from_git(bytes)?;
    // `--path-format=absolute` should make this a no-op; it guards against
    // a Git that prints a path relative to the working directory.
    Ok(if p.is_absolute() { p } else { cwd.join(p) })
}

fn classify_failure(
    cwd: &Path,
    args: &[&str],
    out: &commands::GitOutput,
    first_line: Option<&[u8]>,
) -> GitError {
    // Inside a `.git` directory or a bare repository, rev-parse answers the
    // flags and then fails on --show-toplevel.
    if first_line == Some(b"false") {
        return GitError::NotAWorkTree {
            path: cwd.to_path_buf(),
        };
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Git's message is localized; untranslated setups get the specific
    // error, others still get the generic failure with Git's stderr.
    if stderr.to_ascii_lowercase().contains("not a git repository") {
        return GitError::NotARepository {
            path: cwd.to_path_buf(),
            stderr: stderr.trim().to_owned(),
        };
    }
    commands::failure(args, out)
}
