//! Synchronization (`diffkit::sync`): classification, debouncing, the
//! worker, reconciliation, the diff cache, and the real watcher.

mod support;

mod sync {
    mod cache;
    mod classifier;
    mod consistency;
    mod debounce;
    mod events;
    mod platform_paths;
    mod reconcile;
    mod scheduler;
    mod watch;
    mod watcher;
}
