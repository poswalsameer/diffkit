//! Reconciliation: re-derive repository state from Git.
//!
//! A reconciliation takes a fresh Phase 1 snapshot and the four Phase 2
//! comparison lists, using the same APIs as startup and manual refresh.
//! Watcher events only decide *when* this runs, plus one narrowing that
//! is safe by construction. A list is reused from the previous result only
//! when:
//!
//! - HEAD resolves to the same commit, the repository locations are the
//!   same, and the previous list was discovered successfully; and
//! - the trigger cannot affect that list:
//!   - Staged (HEAD → index) is reused when only working-tree files,
//!     ignore rules, refs, or config changed;
//!   - Unstaged, Combined, and Untracked are reused when only refs or
//!     config changed (a fetch, a new branch, an upstream setting).
//!
//! Manual refreshes, overflows, structural changes, and startup always
//! rediscover everything.
//!
//! The snapshot and lists come from several Git commands run one after
//! another. So that a result never mixes two repository states (a status
//! from before `git add` with lists from after it), reconciliation records
//! a [`StateMarker`] of the index, HEAD, the current branch's ref,
//! `packed-refs`, and operation state before reading, and compares it after
//! the snapshot, after each list, and at the end. As soon as anything
//! moved, the attempt stops (the rest would be discarded anyway) and, after
//! a short pause that lets the Git command finish, starts over, up to
//! [`MAX_ATTEMPTS`] times. The last attempt always reads everything. If Git
//! is still changing then, the result is marked
//! [`Reconciled::settled`]` == false`; the worker does not publish it and
//! runs again once Git goes quiet. Working-tree files are not
//! part of the marker; edits there are caught by the watcher.
//!
//! Git work per reconciliation is kept to what the trigger requires:
//!
//! - Repository discovery (`rev-parse`) is reused from the previous result
//!   unless refs, config, or the repository's structure may have changed.
//! - When only working-tree files, the index, or ignore rules changed,
//!   status skips the ahead/behind walk and the upstream is reused
//!   ([`snapshot::inspect_repository_since`]).
//! - The tracked lists compare only the paths status reported, and the
//!   untracked list comes from status ([`DiffEngine::discover_reported`]).

use super::{classifier::WatchLayout, events::ChangeSet};
use crate::{
    diff::{DiffComparison, DiffEngine, DiffFileList, DiffOptions},
    git::{
        self,
        errors::GitError,
        models::{RepositoryInfo, RepositorySnapshot},
        repository, snapshot,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

/// Attempts per reconciliation when Git state keeps moving underneath it.
pub const MAX_ATTEMPTS: usize = 3;

/// Pause before attempt `n + 1` after attempt `n` saw Git state move.
const RETRY_PAUSE: Duration = Duration::from_millis(10);

/// Triggers that cannot move the repository's locations, shallowness, or
/// object format, so repository discovery can be reused.
const LOCAL_CHANGES: ChangeSet = ChangeSet::WORKTREE
    .union(ChangeSet::IGNORE_RULES)
    .union(ChangeSet::INDEX)
    .union(ChangeSet::HEAD)
    .union(ChangeSet::MERGE_STATE)
    .union(ChangeSet::REBASE_STATE);

/// Triggers that cannot change HEAD, refs, or the upstream, so the upstream
/// and ahead/behind counts can be reused.
const NO_REF_CHANGES: ChangeSet = ChangeSet::WORKTREE
    .union(ChangeSet::IGNORE_RULES)
    .union(ChangeSet::INDEX);

/// Metadata of one file: absent, or (length, modification time, inode).
type Stamp = Option<(u64, Option<SystemTime>, u64)>;

fn stamp(p: &Path) -> Stamp {
    let m = fs::symlink_metadata(p).ok()?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let inode = 0;
    Some((m.len(), m.modified().ok(), inode))
}

/// The Git metadata files whose change would make a snapshot and the lists
/// read after it disagree. Reading them needs no Git process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMarker {
    head: Option<Vec<u8>>,
    files: Vec<Stamp>,
}

impl StateMarker {
    pub fn read(repo: &RepositoryInfo) -> Self {
        let head = fs::read(repo.git_dir.join("HEAD")).ok();
        let mut paths = vec![
            repo.git_dir.join("index"),
            repo.common_git_dir.join("packed-refs"),
            repo.common_git_dir.join("reftable").join("tables.list"),
        ];
        // The branch HEAD points at (`ref: refs/heads/main`). Per-worktree
        // refs live in the Git directory, shared ones in the common one.
        if let Some(name) = head
            .as_deref()
            .and_then(|h| h.strip_prefix(b"ref: "))
            .and_then(|r| std::str::from_utf8(r).ok())
            .map(str::trim)
            .filter(|r| r.starts_with("refs/") && !r.split('/').any(|c| c == ".."))
        {
            paths.push(repo.git_dir.join(name));
            paths.push(repo.common_git_dir.join(name));
        }
        for state in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "rebase-merge",
            "rebase-apply",
        ] {
            paths.push(repo.git_dir.join(state));
        }
        Self {
            head,
            files: paths.iter().map(|p| stamp(p)).collect(),
        }
    }
}

/// One comparison's file list, shared between the worker and the interface.
pub type ListResult = Result<Arc<DiffFileList>, String>;

/// A complete, consistent result ready to be published.
#[derive(Debug, Clone)]
pub struct Reconciled {
    /// Shared, so keeping a result as the base for the next one is cheap.
    pub snapshot: Arc<RepositorySnapshot>,
    /// In [`DiffComparison::ALL`] order.
    pub lists: Vec<ListResult>,
    pub trigger: ChangeSet,
    pub duration: Duration,
    /// Lists reused from the previous result.
    pub reused: usize,
    /// Attempts discarded because Git state moved while they were read.
    pub retries: usize,
    /// Nothing moved while the published attempt was read. `false` only
    /// when Git kept changing through all [`MAX_ATTEMPTS`]; the result may
    /// then mix two states, and the worker schedules another full run.
    pub settled: bool,
}

impl Reconciled {
    /// Same repository state as `other` (ignoring how it was obtained).
    pub fn same_state(&self, other: &Self) -> bool {
        self.snapshot == other.snapshot
            && self.lists.len() == other.lists.len()
            && self
                .lists
                .iter()
                .zip(&other.lists)
                .all(|(a, b)| match (a, b) {
                    (Ok(a), Ok(b)) => Arc::ptr_eq(a, b) || a == b,
                    (Err(a), Err(b)) => a == b,
                    _ => false,
                })
    }
}

fn reusable(c: DiffComparison, trigger: ChangeSet) -> bool {
    let staged_safe =
        ChangeSet::WORKTREE | ChangeSet::IGNORE_RULES | ChangeSet::REFS | ChangeSet::CONFIG;
    let refs_only = ChangeSet::REFS | ChangeSet::CONFIG;
    match c {
        DiffComparison::Staged => trigger.within(staged_safe),
        _ => trigger.within(refs_only),
    }
}

/// Builds a full result for the repository containing `start`.
pub fn reconcile(
    start: &Path,
    options: DiffOptions,
    trigger: ChangeSet,
    previous: Option<&Reconciled>,
) -> Result<Reconciled, GitError> {
    let began = Instant::now();
    let known = previous
        .filter(|_| trigger.within(LOCAL_CHANGES))
        .map(|p| p.snapshot.repository.clone());
    let reusing = known.is_some();
    let mut repo = match known {
        Some(r) => r,
        None => repository::discover_from(start)?,
    };
    let mut rediscovered = false;
    let mut retries = 0;
    loop {
        let before = StateMarker::read(&repo);
        let last = retries + 1 >= MAX_ATTEMPTS;
        let attempt = match read_state(&repo, options, trigger, previous, &before, !last) {
            // The reused locations may be out of date (a repository moved
            // without a structural event): discover once more and retry.
            Err(_) if reusing && !rediscovered => {
                rediscovered = true;
                repo = repository::discover_from(start)?;
                continue;
            }
            other => other?,
        };
        // `None`: Git state moved partway and the attempt stopped early.
        let moved = attempt.is_none() || StateMarker::read(&repo) != before;
        match attempt {
            Some((snapshot, lists, reused)) if !moved || last => {
                return Ok(Reconciled {
                    snapshot: Arc::new(snapshot),
                    lists,
                    trigger,
                    duration: began.elapsed(),
                    reused,
                    retries,
                    settled: !moved,
                })
            }
            _ => {
                retries += 1;
                std::thread::sleep(RETRY_PAUSE * retries as u32);
            }
        }
    }
}

/// The snapshot, the lists in [`DiffComparison::ALL`] order, and how many
/// lists were reused.
type State = (RepositorySnapshot, Vec<ListResult>, usize);

/// One attempt: the snapshot, then each list (reused when allowed). With
/// `stop_early`, returns `None` as soon as the Git state differs from
/// `before`, checked after the snapshot and after each list.
fn read_state(
    repo: &RepositoryInfo,
    options: DiffOptions,
    trigger: ChangeSet,
    previous: Option<&Reconciled>,
    before: &StateMarker,
    stop_early: bool,
) -> Result<Option<State>, GitError> {
    let moved = || stop_early && StateMarker::read(repo) != *before;
    let comparable = previous.filter(|p| {
        p.snapshot.repository.root == repo.root && p.snapshot.repository.git_dir == repo.git_dir
    });
    let snapshot = match comparable.filter(|_| trigger.within(NO_REF_CHANGES)) {
        Some(p) => snapshot::inspect_repository_since(repo.clone(), &p.snapshot)?,
        None => snapshot::inspect_repository(repo.clone())?,
    };
    if moved() {
        return Ok(None);
    }
    let engine = DiffEngine::from_snapshot(&snapshot, options);
    let same_head =
        comparable.filter(|p| p.snapshot.branch.head_commit == snapshot.branch.head_commit);
    let mut reused = 0;
    let mut lists = Vec::with_capacity(DiffComparison::ALL.len());
    for (i, &c) in DiffComparison::ALL.iter().enumerate() {
        if let Some(Ok(prev)) = same_head.and_then(|p| p.lists.get(i)) {
            if reusable(c, trigger) {
                reused += 1;
                lists.push(Ok(Arc::clone(prev)));
                continue;
            }
        }
        lists.push(
            engine
                .discover_reported(c, &snapshot.status)
                .map(Arc::new)
                .map_err(|e| e.to_string()),
        );
        if moved() {
            return Ok(None);
        }
    }
    Ok(Some((snapshot, lists, reused)))
}

/// Untracked directories that are themselves ignored, as absolute paths.
///
/// `git ls-files --others --ignored --exclude-standard --directory` lists
/// candidates, but also directories whose *current* contents merely happen
/// to be ignored (a `web/` holding only `web/node_modules/`); a new file
/// there would matter. `git check-ignore --no-index` then keeps only
/// directories matched by an ignore rule. Git never re-includes anything
/// below an ignored directory, so changes inside one cannot affect any
/// comparison. Two Git calls; none when there are no candidates.
pub fn ignored_directories(root: &Path) -> Result<Vec<PathBuf>, GitError> {
    let out = git::commands::run(
        root,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
    )?;
    let candidates: Vec<&[u8]> = out
        .split(|&b| b == 0)
        .filter(|e| e.len() > 1 && e.ends_with(b"/"))
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    // `-z` requires `--stdin`, which the command layer never provides.
    // Without it, Git C-quotes any path with special characters, so each
    // output line is still unambiguous. check-ignore rejects
    // GIT_LITERAL_PATHSPECS, so pathspec magic is neutralized with a `./`
    // prefix instead (Git echoes the argument as given).
    let mut args: Vec<std::ffi::OsString> = ["check-ignore", "--no-index", "--"]
        .iter()
        .map(Into::into)
        .collect();
    for c in &candidates {
        let mut arg = std::ffi::OsString::from("./");
        arg.push(git::commands::path_from_git(c)?.as_os_str());
        args.push(arg);
    }
    let checked = git::commands::output_without_literal_pathspecs(root, &args)?;
    // Exit 1 means "nothing is ignored"; only other codes are failures.
    if !matches!(checked.status.code(), Some(0) | Some(1)) {
        return Err(git::commands::failure(&args, &checked));
    }
    let mut dirs = Vec::new();
    for line in git::commands::strip_newline(&checked.stdout).split(|&b| b == b'\n') {
        let raw = if line.first() == Some(&b'"') {
            match crate::diff::parser::unquote(line) {
                Some((p, used)) if used == line.len() => p,
                _ => {
                    return Err(GitError::malformed(
                        "check-ignore",
                        String::from_utf8_lossy(line).into_owned(),
                    ))
                }
            }
        } else {
            line.to_vec()
        };
        let raw = raw.strip_prefix(b"./").unwrap_or(&raw);
        let dir = raw.strip_suffix(b"/").unwrap_or(raw);
        if !dir.is_empty() {
            dirs.push(root.join(git::commands::path_from_git(dir)?));
        }
    }
    Ok(dirs)
}

/// Builds the watch layout for a reconciled repository.
pub fn layout_for(r: &Reconciled, excluded: Vec<PathBuf>) -> WatchLayout {
    let mut layout = WatchLayout::new(&r.snapshot.repository);
    layout.excluded = excluded;
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuse_rules() {
        use DiffComparison::*;
        let w = ChangeSet::WORKTREE;
        assert!(reusable(Staged, w));
        assert!(!reusable(Unstaged, w));
        assert!(!reusable(Untracked, w));
        assert!(reusable(Combined, ChangeSet::REFS | ChangeSet::CONFIG));
        for never in [
            ChangeSet::INDEX,
            ChangeSet::HEAD,
            ChangeSet::MERGE_STATE,
            ChangeSet::REBASE_STATE,
            ChangeSet::STRUCTURE,
            ChangeSet::OVERFLOW,
            ChangeSet::MANUAL,
            ChangeSet::STARTUP,
        ] {
            for c in DiffComparison::ALL {
                assert!(!reusable(c, never | ChangeSet::REFS), "{c:?} {never:?}");
            }
        }
    }
}
