//! Filesystem watching with `notify`.
//!
//! What is watched:
//! - the working tree, recursively;
//! - when they lie outside the working tree (linked worktrees, separate Git
//!   directories): the per-worktree Git directory (HEAD, index, merge and
//!   rebase state) and the common directory (packed-refs, config), both
//!   non-recursively, plus the common `refs/` and `info/` recursively.
//!
//! The object database, logs, and hooks are never watched on their own.
//! When the Git directory is inside the working tree, the recursive watch
//! covers it and the classifier drops irrelevant paths.
//!
//! Symlinks are not followed. The event handler runs on notify's thread and
//! only does a non-blocking `try_send` into the worker's bounded queue; a
//! full queue increments a counter instead of blocking or growing.

use super::{classifier::WatchLayout, scheduler::Msg};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError},
        Arc,
    },
};

pub struct FsWatcher {
    _watcher: RecommendedWatcher,
    pub watched: Vec<(PathBuf, RecursiveMode)>,
}

impl std::fmt::Debug for FsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsWatcher")
            .field("watched", &self.watched)
            .finish()
    }
}

/// Paths to watch for a layout, without duplicates already covered by the
/// recursive working-tree watch.
pub fn watch_plan(layout: &WatchLayout) -> Vec<(PathBuf, RecursiveMode)> {
    let covered = |p: &Path| p.starts_with(&layout.root);
    let mut plan = vec![(layout.root.clone(), RecursiveMode::Recursive)];
    if !covered(&layout.git_dir) {
        plan.push((layout.git_dir.clone(), RecursiveMode::NonRecursive));
    }
    if layout.common_dir != layout.git_dir && !covered(&layout.common_dir) {
        plan.push((layout.common_dir.clone(), RecursiveMode::NonRecursive));
        for sub in ["refs", "info"] {
            let p = layout.common_dir.join(sub);
            if p.is_dir() {
                plan.push((p, RecursiveMode::Recursive));
            }
        }
    }
    plan
}

/// The notification callback: never blocks and never grows a queue. When
/// the worker's queue is full the event is counted as dropped; the worker
/// then treats the situation as an overflow and reconciles fully.
pub fn handler(
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
) -> impl Fn(notify::Result<notify::Event>) + Send + 'static {
    move |res| match tx.try_send(Msg::Fs(res)) {
        Ok(()) | Err(TrySendError::Disconnected(_)) => {}
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn start(
    layout: &WatchLayout,
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
) -> notify::Result<FsWatcher> {
    let mut watcher = RecommendedWatcher::new(
        handler(tx, dropped),
        Config::default().with_follow_symlinks(false),
    )?;
    let plan = watch_plan(layout);
    for (path, mode) in &plan {
        watcher.watch(path, *mode)?;
    }
    Ok(FsWatcher {
        _watcher: watcher,
        watched: plan,
    })
}
