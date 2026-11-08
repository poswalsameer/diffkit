//! Debouncing with a quiet period and a maximum delay.
//!
//! Time is passed in by the caller (monotonic `Instant`s), so the logic is
//! deterministic and tested without sleeping. Pending state is one
//! [`ChangeSet`] and two instants, whatever the number of events.

use super::events::ChangeSet;
use std::time::{Duration, Instant};

pub const DEFAULT_QUIET: Duration = Duration::from_millis(200);
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub changes: ChangeSet,
    /// Relevant events merged into this batch.
    pub events: u64,
}

#[derive(Debug, Clone)]
pub struct Debouncer {
    quiet: Duration,
    max_delay: Duration,
    pending: ChangeSet,
    events: u64,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Debouncer {
    pub fn new(quiet: Duration, max_delay: Duration) -> Self {
        Self {
            quiet,
            max_delay: max_delay.max(quiet),
            pending: ChangeSet::NONE,
            events: 0,
            first: None,
            last: None,
        }
    }

    /// Records relevant changes seen at `now`. Returns true if this started
    /// a new pending batch.
    pub fn record(&mut self, changes: ChangeSet, now: Instant) -> bool {
        if changes.is_empty() {
            return false;
        }
        let started = self.first.is_none();
        self.pending.insert(changes);
        self.events += 1;
        self.first.get_or_insert(now);
        self.last = Some(now);
        started
    }

    pub fn is_pending(&self) -> bool {
        self.first.is_some()
    }

    pub fn pending(&self) -> ChangeSet {
        self.pending
    }

    /// When the pending batch becomes due: a quiet period after the last
    /// event, but never later than the maximum delay after the first.
    pub fn deadline(&self) -> Option<Instant> {
        let (first, last) = (self.first?, self.last?);
        Some((last + self.quiet).min(first + self.max_delay))
    }

    /// Takes the pending batch if it is due at `now`.
    pub fn take_if_due(&mut self, now: Instant) -> Option<Batch> {
        if self.deadline()? <= now {
            self.take()
        } else {
            None
        }
    }

    /// Takes whatever is pending, due or not.
    pub fn take(&mut self) -> Option<Batch> {
        self.first?;
        let batch = Batch {
            changes: self.pending,
            events: self.events,
        };
        *self = Self::new(self.quiet, self.max_delay);
        Some(batch)
    }
}
