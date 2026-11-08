//! One module per command. Each `run` returns the process exit code; the
//! plain-text commands also have `render`, which builds their output
//! without printing it. Only [`inspect`] is public: its report is rendered
//! from constructed snapshots in tests; the others are reached through
//! [`super::run`].

pub(super) mod diff_stats;
pub mod inspect;
pub(super) mod interactive;
pub(super) mod show;
