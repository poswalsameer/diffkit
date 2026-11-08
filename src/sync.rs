//! Phase 4 synchronization: keeps the interface current when the
//! repository changes outside Diffkit.
//!
//! ```text
//! notify event ─▶ bounded queue ─▶ classify ─▶ debounce ─▶ reconcile (Git)
//!                                                              │
//!                         interface ◀── SyncEvent (whole new state) ┘
//! ```
//!
//! Filesystem events are only hints. What changed is always re-derived from
//! Git through the Phase 1 snapshot and Phase 2 discovery APIs.

pub mod cache;
pub mod classifier;
pub mod debounce;
pub mod events;
pub mod reconcile;
pub mod scheduler;
pub mod watcher;
