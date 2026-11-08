//! The watcher's plan (what is watched, recursively or not) and its
//! never-blocking event handler.

use diffkit::sync::{classifier::WatchLayout, watcher::*};
use notify::RecursiveMode;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[test]
fn full_queue_counts_drops_instead_of_blocking() {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let dropped = Arc::new(AtomicU64::new(0));
    let h = handler(tx, Arc::clone(&dropped));
    for _ in 0..3 {
        h(Ok(notify::Event::new(notify::EventKind::Any)));
    }
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
    assert!(rx.try_recv().is_ok());
    drop(rx);
    h(Ok(notify::Event::new(notify::EventKind::Any)));
    assert_eq!(
        dropped.load(Ordering::Relaxed),
        2,
        "a closed queue is not an overflow"
    );
}

#[test]
fn standard_repository_needs_one_recursive_watch() {
    let l = WatchLayout {
        root: "/r".into(),
        git_dir: "/r/.git".into(),
        common_dir: "/r/.git".into(),
        excluded: vec![],
    };
    assert_eq!(
        watch_plan(&l),
        vec![(PathBuf::from("/r"), RecursiveMode::Recursive)]
    );
}

#[test]
fn linked_worktree_watches_its_metadata_but_not_objects() {
    let l = WatchLayout {
        root: "/wt".into(),
        git_dir: "/main/.git/worktrees/wt".into(),
        common_dir: "/main/.git".into(),
        excluded: vec![],
    };
    let plan = watch_plan(&l);
    assert!(plan.contains(&(PathBuf::from("/wt"), RecursiveMode::Recursive)));
    assert!(plan.contains(&(
        PathBuf::from("/main/.git/worktrees/wt"),
        RecursiveMode::NonRecursive
    )));
    assert!(plan.contains(&(PathBuf::from("/main/.git"), RecursiveMode::NonRecursive)));
    assert!(!plan
        .iter()
        .any(|(p, m)| p == Path::new("/main/.git") && *m == RecursiveMode::Recursive));
    assert!(!plan.iter().any(|(p, _)| p.ends_with("objects")));
}
