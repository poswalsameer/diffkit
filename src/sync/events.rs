//! Logical change categories and synchronization diagnostics.

use std::{fmt, time::Duration};

/// A set of logical change categories. A fixed-size bit set: however many
/// filesystem events arrive, pending state never grows.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ChangeSet(u16);

impl ChangeSet {
    pub const NONE: Self = Self(0);
    /// A file or directory in the working tree changed.
    pub const WORKTREE: Self = Self(1);
    /// A `.gitignore` or `info/exclude` changed.
    pub const IGNORE_RULES: Self = Self(1 << 1);
    pub const INDEX: Self = Self(1 << 2);
    pub const HEAD: Self = Self(1 << 3);
    /// Branch, tag, remote-tracking refs, or `packed-refs`.
    pub const REFS: Self = Self(1 << 4);
    /// Repository configuration (upstream settings live there).
    pub const CONFIG: Self = Self(1 << 5);
    /// Merge, cherry-pick, revert, or bisect state.
    pub const MERGE_STATE: Self = Self(1 << 6);
    pub const REBASE_STATE: Self = Self(1 << 7);
    /// The root, `.git`, or a Git directory itself was replaced or removed.
    pub const STRUCTURE: Self = Self(1 << 8);
    /// Events were lost; nothing can be assumed about what changed.
    pub const OVERFLOW: Self = Self(1 << 9);
    /// The user asked for a refresh.
    pub const MANUAL: Self = Self(1 << 10);
    /// First reconciliation after the watcher started.
    pub const STARTUP: Self = Self(1 << 11);

    const NAMES: [(Self, &'static str); 12] = [
        (Self::WORKTREE, "worktree"),
        (Self::IGNORE_RULES, "ignore-rules"),
        (Self::INDEX, "index"),
        (Self::HEAD, "head"),
        (Self::REFS, "refs"),
        (Self::CONFIG, "config"),
        (Self::MERGE_STATE, "merge-state"),
        (Self::REBASE_STATE, "rebase-state"),
        (Self::STRUCTURE, "structure"),
        (Self::OVERFLOW, "overflow"),
        (Self::MANUAL, "manual"),
        (Self::STARTUP, "startup"),
    ];

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Every category in `self` is also in `allowed`.
    pub fn within(self, allowed: Self) -> bool {
        self.0 & !allowed.0 == 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(f, _)| self.contains(*f))
            .map(|(_, n)| *n)
            .collect()
    }
}

impl std::ops::BitOr for ChangeSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl fmt::Debug for ChangeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChangeSet({})", self.names().join("|"))
    }
}

/// Counters kept by the synchronization worker. Cheap to copy; sent with
/// every update so the interface can show them on request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncDiagnostics {
    /// Filesystem notifications received from the watcher.
    pub fs_events: u64,
    /// Notifications classified as relevant.
    pub relevant_events: u64,
    /// Relevant notifications merged into an already pending request.
    pub coalesced_events: u64,
    /// Times the bounded event queue was full.
    pub overflows: u64,
    /// Notifications dropped because the queue was full.
    pub dropped_events: u64,
    pub reconcile_requests: u64,
    pub reconciles_completed: u64,
    /// Completed reconciliations whose result equalled the previous state.
    pub reconciles_unchanged: u64,
    pub reconcile_failures: u64,
    /// Comparison lists reused instead of rediscovered.
    pub lists_reused: u64,
    /// Times a reconciliation started over because the index, HEAD, refs,
    /// or operation state changed while it was reading.
    pub reconcile_retries: u64,
    /// Results not published because Git kept changing through every
    /// attempt (a full run follows).
    pub reconciles_withheld: u64,
    pub last_reconcile: Option<Duration>,
    pub total_reconcile: Duration,
    /// Git processes started by the synchronization worker.
    pub git_calls: u64,
    pub watcher_errors: u64,
    pub watcher_restarts: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_operations() {
        let s = ChangeSet::WORKTREE | ChangeSet::INDEX;
        assert!(s.contains(ChangeSet::INDEX));
        assert!(!s.contains(ChangeSet::HEAD));
        assert!(s.intersects(ChangeSet::INDEX | ChangeSet::HEAD));
        assert!(s.within(ChangeSet::WORKTREE | ChangeSet::INDEX | ChangeSet::REFS));
        assert!(!s.within(ChangeSet::WORKTREE));
        assert!(ChangeSet::NONE.is_empty() && ChangeSet::NONE.within(ChangeSet::NONE));
        assert_eq!(s.names(), vec!["worktree", "index"]);
        assert_eq!(format!("{s:?}"), "ChangeSet(worktree|index)");
    }
}
