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
use crate::git::models::RepositoryInfo;
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

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, Flag, ModifyKind, RemoveKind, RenameMode};

    fn standard() -> WatchLayout {
        WatchLayout {
            root: "/r".into(),
            git_dir: "/r/.git".into(),
            common_dir: "/r/.git".into(),
            excluded: vec!["/r/target".into(), "/r/web/node_modules".into()],
        }
    }

    fn linked() -> WatchLayout {
        WatchLayout {
            root: "/wt".into(),
            git_dir: "/main/.git/worktrees/wt".into(),
            common_dir: "/main/.git".into(),
            excluded: vec![],
        }
    }

    fn ev(kind: EventKind, paths: &[&str]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        e
    }

    fn modify(paths: &[&str]) -> Event {
        ev(EventKind::Modify(ModifyKind::Any), paths)
    }

    #[test]
    fn working_tree_changes() {
        let l = standard();
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
            EventKind::Create(CreateKind::Folder),
            EventKind::Any,
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
        ] {
            assert_eq!(
                classify(&l, &ev(kind, &["/r/src/main.rs"])),
                ChangeSet::WORKTREE,
                "{kind:?}"
            );
        }
        let rename = ev(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/r/src/old.rs", "/r/src/new.rs"],
        );
        assert_eq!(classify(&l, &rename), ChangeSet::WORKTREE);
        // Atomic save: temporary file renamed over the original.
        let atomic = ev(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/r/src/.main.rs.swp", "/r/src/main.rs"],
        );
        assert_eq!(classify(&l, &atomic), ChangeSet::WORKTREE);
        assert_eq!(
            classify(&l, &modify(&["/r/.gitignore"])),
            ChangeSet::WORKTREE | ChangeSet::IGNORE_RULES
        );
    }

    #[test]
    fn reads_are_irrelevant() {
        let l = standard();
        let read = ev(EventKind::Access(AccessKind::Read), &["/r/src/main.rs"]);
        assert_eq!(classify(&l, &read), ChangeSet::NONE);
    }

    #[test]
    fn git_metadata() {
        let l = standard();
        let cases = [
            ("/r/.git/index", ChangeSet::INDEX),
            ("/r/.git/index.lock", ChangeSet::INDEX),
            ("/r/.git/HEAD", ChangeSet::HEAD),
            ("/r/.git/refs/heads/feature/x", ChangeSet::REFS),
            ("/r/.git/refs/remotes/origin/main", ChangeSet::REFS),
            ("/r/.git/packed-refs", ChangeSet::REFS),
            ("/r/.git/packed-refs.lock", ChangeSet::REFS),
            ("/r/.git/config", ChangeSet::CONFIG),
            ("/r/.git/MERGE_HEAD", ChangeSet::MERGE_STATE),
            ("/r/.git/CHERRY_PICK_HEAD", ChangeSet::MERGE_STATE),
            ("/r/.git/REVERT_HEAD", ChangeSet::MERGE_STATE),
            ("/r/.git/BISECT_LOG", ChangeSet::MERGE_STATE),
            ("/r/.git/BISECT_START", ChangeSet::MERGE_STATE),
            ("/r/.git/refs/bisect/bad", ChangeSet::REFS),
            ("/r/.git/rebase-merge/done", ChangeSet::REBASE_STATE),
            ("/r/.git/rebase-apply", ChangeSet::REBASE_STATE),
            (
                "/r/.git/info/exclude",
                ChangeSet::WORKTREE | ChangeSet::IGNORE_RULES,
            ),
            ("/r/.git/modules/sub/HEAD", ChangeSet::WORKTREE),
            ("/r/.git", ChangeSet::STRUCTURE),
        ];
        for (p, want) in cases {
            assert_eq!(classify(&l, &modify(&[p])), want, "{p}");
        }
    }

    #[test]
    fn irrelevant_git_internals_and_ignored_directories() {
        let l = standard();
        for p in [
            "/r/.git/objects/ab/cdef0123",
            "/r/.git/objects/pack/pack-1.pack",
            "/r/.git/logs/HEAD",
            "/r/.git/hooks/pre-commit",
            "/r/.git/FETCH_HEAD",
            "/r/.git/ORIG_HEAD",
            "/r/.git/COMMIT_EDITMSG",
            "/r/.git/modules/sub/objects/ab/cd",
            "/r/target/debug/build.o",
            "/r/web/node_modules/pkg/index.js",
            "/r/vendor/nested/.git/index",
            "/elsewhere/file",
        ] {
            assert_eq!(classify(&l, &modify(&[p])), ChangeSet::NONE, "{p}");
        }
        // Only directories Git reports as ignored are excluded: a `target`
        // elsewhere is ordinary content.
        assert_eq!(
            classify(&l, &modify(&["/r/crates/x/target/a.rs"])),
            ChangeSet::WORKTREE
        );
    }

    #[test]
    fn linked_worktree_metadata() {
        let l = linked();
        let cases = [
            ("/main/.git/worktrees/wt/HEAD", ChangeSet::HEAD),
            ("/main/.git/worktrees/wt/index", ChangeSet::INDEX),
            ("/main/.git/worktrees/wt/MERGE_HEAD", ChangeSet::MERGE_STATE),
            ("/main/.git/refs/heads/main", ChangeSet::REFS),
            ("/main/.git/packed-refs", ChangeSet::REFS),
            ("/main/.git/config", ChangeSet::CONFIG),
            // The main worktree's own HEAD, index, and merge state do not
            // affect this worktree.
            ("/main/.git/HEAD", ChangeSet::NONE),
            ("/main/.git/index", ChangeSet::NONE),
            ("/main/.git/MERGE_HEAD", ChangeSet::NONE),
            ("/main/.git/worktrees/other/HEAD", ChangeSet::NONE),
            ("/wt/.git", ChangeSet::STRUCTURE),
            ("/wt/src/a.rs", ChangeSet::WORKTREE),
        ];
        for (p, want) in cases {
            assert_eq!(classify(&l, &modify(&[p])), want, "{p}");
        }
    }

    #[test]
    fn rescan_flag_means_overflow() {
        let l = standard();
        let e = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(classify(&l, &e), ChangeSet::OVERFLOW);
        let e = modify(&["/r/a"]).set_flag(Flag::Rescan);
        assert_eq!(classify(&l, &e), ChangeSet::OVERFLOW | ChangeSet::WORKTREE);
    }

    #[test]
    fn root_removal_is_structural() {
        let l = standard();
        assert_eq!(
            classify(&l, &ev(EventKind::Remove(RemoveKind::Folder), &["/r"])),
            ChangeSet::STRUCTURE
        );
    }
}
