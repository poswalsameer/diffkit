//! Turns filesystem notifications into logical change categories.
//!
//! Classification only decides whether something *might* have changed and
//! roughly where. It never decides what changed: that is always
//! re-derived from Git by reconciliation.
//!
//! Locations come from the Phase 1 repository model, so linked worktrees
//! (where `.git` is a file) and separate Git directories work. The
//! per-worktree Git directory is checked before the common directory,
//! because a linked worktree's Git directory lives inside it.

use super::events::ChangeSet;
use crate::types::repository::RepositoryInfo;
use notify::{
    event::{AccessKind, AccessMode},
    Event, EventKind,
};
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

/// Where a repository's files and metadata live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchLayout {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    /// Untracked directories Git ignores in their entirety (no tracked file
    /// inside), from `git ls-files --others --ignored --directory`. Events
    /// inside them cannot change any comparison.
    pub excluded: Vec<PathBuf>,
}

impl WatchLayout {
    pub fn new(info: &RepositoryInfo) -> Self {
        Self {
            root: info.root.clone(),
            git_dir: info.git_dir.clone(),
            common_dir: info.common_git_dir.clone(),
            excluded: Vec::new(),
        }
    }

    /// Same repository locations (exclusions aside).
    pub fn same_locations(&self, other: &Self) -> bool {
        self.root == other.root
            && self.git_dir == other.git_dir
            && self.common_dir == other.common_dir
    }

    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }
}

/// Categories one notification may affect. `NONE` means irrelevant.
pub fn classify(layout: &WatchLayout, event: &Event) -> ChangeSet {
    let mut out = ChangeSet::NONE;
    if event.need_rescan() {
        out.insert(ChangeSet::OVERFLOW);
    }
    match event.kind {
        // Reads and opens never change content; a close after writing does.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
        EventKind::Access(_) => return out,
        _ => {}
    }
    for path in &event.paths {
        out.insert(classify_path(layout, path));
    }
    out
}

pub fn classify_path(layout: &WatchLayout, path: &Path) -> ChangeSet {
    if let Ok(rel) = path.strip_prefix(&layout.git_dir) {
        return git_dir_entry(rel, true);
    }
    if let Ok(rel) = path.strip_prefix(&layout.common_dir) {
        return git_dir_entry(rel, !layout.is_linked_worktree());
    }
    let Ok(rel) = path.strip_prefix(&layout.root) else {
        return ChangeSet::NONE;
    };
    if rel.as_os_str().is_empty() || rel == Path::new(".git") {
        // The root itself, or a linked worktree's `.git` file.
        return ChangeSet::STRUCTURE;
    }
    if rel
        .components()
        .any(|c| c == Component::Normal(OsStr::new(".git")))
    {
        // Internals of a nested repository.
        return ChangeSet::NONE;
    }
    if layout.excluded.iter().any(|d| path.starts_with(d)) {
        return ChangeSet::NONE;
    }
    if path.file_name() == Some(OsStr::new(".gitignore")) {
        return ChangeSet::WORKTREE | ChangeSet::IGNORE_RULES;
    }
    ChangeSet::WORKTREE
}

/// `rel` is relative to a Git directory. `own` is true when that directory
/// holds this worktree's HEAD, index, and operation state.
fn git_dir_entry(rel: &Path, own: bool) -> ChangeSet {
    let mut parts = rel.components().filter_map(|c| match c {
        Component::Normal(s) => s.to_str(),
        _ => None,
    });
    let Some(first) = parts.next() else {
        return ChangeSet::STRUCTURE;
    };
    let first = first.strip_suffix(".lock").unwrap_or(first);
    let rest: Vec<&str> = parts.collect();
    let per_worktree = |set: ChangeSet| if own { set } else { ChangeSet::NONE };
    match first {
        "index" => per_worktree(ChangeSet::INDEX),
        f if f.starts_with("sharedindex.") => per_worktree(ChangeSet::INDEX),
        "HEAD" => per_worktree(ChangeSet::HEAD),
        "MERGE_HEAD" | "MERGE_MSG" | "MERGE_MODE" | "MERGE_RR" | "AUTO_MERGE"
        | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "sequencer" => per_worktree(ChangeSet::MERGE_STATE),
        // Bisect state is per worktree; it is reported like a merge.
        f if f.starts_with("BISECT_") => per_worktree(ChangeSet::MERGE_STATE),
        "rebase-merge" | "rebase-apply" | "REBASE_HEAD" => per_worktree(ChangeSet::REBASE_STATE),
        "packed-refs" | "refs" | "shallow" => ChangeSet::REFS,
        "config" | "config.worktree" => ChangeSet::CONFIG,
        "info" if rest.first() == Some(&"exclude") => ChangeSet::WORKTREE | ChangeSet::IGNORE_RULES,
        // Submodule Git directories: their HEAD, index, and refs change what
        // the parent reports for the submodule.
        "modules" => match rest.last() {
            Some(&"HEAD") | Some(&"index") | Some(&"packed-refs") => ChangeSet::WORKTREE,
            _ if rest.contains(&"refs") => ChangeSet::WORKTREE,
            _ => ChangeSet::NONE,
        },
        // objects, logs, hooks, worktrees/<others>, FETCH_HEAD, ORIG_HEAD,
        // COMMIT_EDITMSG, gc files, lfs, …: no effect on any comparison.
        _ => ChangeSet::NONE,
    }
}
