//! Event classification: which Git directory and working-tree paths map to
//! which change categories, per worktree, and which are ignored.

use diffkit::sync::{classifier::*, events::ChangeSet};
use notify::{
    event::{AccessKind, AccessMode, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode},
    Event, EventKind,
};
use std::path::PathBuf;

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
