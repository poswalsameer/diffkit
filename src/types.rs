//! Data types shared between features. Each feature's own types stay in
//! that feature.
//!
//! - [`repository`]: repository state from Git (snapshot, branch, status,
//!   conflicts, operations), produced by [`crate::git`] and read by every
//!   other feature.
//! - [`diff`]: changed-file metadata, loaded diffs, and aligned rows,
//!   produced by [`crate::diff`] and read by [`crate::sync`],
//!   [`crate::tui`], and [`crate::cli`].

pub mod diff;
pub mod repository;
