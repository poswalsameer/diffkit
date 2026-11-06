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

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn d() -> (Debouncer, Instant) {
        (Debouncer::new(ms(200), ms(1000)), Instant::now())
    }

    #[test]
    fn single_event_fires_after_quiet_period() {
        let (mut d, t0) = d();
        assert!(d.record(ChangeSet::WORKTREE, t0));
        assert_eq!(d.deadline(), Some(t0 + ms(200)));
        assert_eq!(d.take_if_due(t0 + ms(199)), None);
        let b = d.take_if_due(t0 + ms(200)).unwrap();
        assert_eq!(
            b,
            Batch {
                changes: ChangeSet::WORKTREE,
                events: 1
            }
        );
        assert!(!d.is_pending());
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn duplicates_and_bursts_merge_into_one_batch() {
        let (mut d, t0) = d();
        assert!(d.record(ChangeSet::WORKTREE, t0));
        for i in 1..100 {
            assert!(
                !d.record(ChangeSet::WORKTREE, t0 + ms(i)),
                "only the first starts a batch"
            );
        }
        d.record(ChangeSet::INDEX, t0 + ms(100));
        assert_eq!(
            d.take_if_due(t0 + ms(250)),
            None,
            "quiet period restarts with each event"
        );
        let b = d.take_if_due(t0 + ms(300)).unwrap();
        assert_eq!(b.changes, ChangeSet::WORKTREE | ChangeSet::INDEX);
        assert_eq!(b.events, 101);
    }

    #[test]
    fn continuous_activity_fires_at_maximum_delay() {
        let (mut d, t0) = d();
        let mut fired = Vec::new();
        // An event every 50 ms for 3.5 s: the quiet period never elapses.
        for i in 0..70 {
            let now = t0 + ms(50 * i);
            if let Some(b) = d.take_if_due(now) {
                fired.push((now - t0, b.events));
            }
            d.record(ChangeSet::WORKTREE, now);
        }
        assert_eq!(fired.len(), 3, "{fired:?}");
        assert_eq!(fired[0].0, ms(1000));
        assert!(fired.iter().all(|(_, n)| *n == 20));
    }

    #[test]
    fn empty_changes_are_ignored() {
        let (mut d, t0) = d();
        assert!(!d.record(ChangeSet::NONE, t0));
        assert!(!d.is_pending());
        assert_eq!(d.take(), None);
    }

    #[test]
    fn take_returns_pending_regardless_of_deadline() {
        let (mut d, t0) = d();
        d.record(ChangeSet::OVERFLOW, t0);
        assert_eq!(d.take().unwrap().changes, ChangeSet::OVERFLOW);
        assert_eq!(d.take(), None);
    }

    #[test]
    fn maximum_delay_is_never_shorter_than_quiet() {
        let mut d = Debouncer::new(ms(300), ms(100));
        let t0 = Instant::now();
        d.record(ChangeSet::WORKTREE, t0);
        assert_eq!(d.deadline(), Some(t0 + ms(300)));
    }
}
