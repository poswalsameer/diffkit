//! Phase 2 diff engine: discovery, lazy per-file loading, unified diff
//! parsing, and Myers-based side-by-side alignment. See
//! `docs/phases/PHASE_2_DIFF_ENGINE.md`.

pub mod alignment;
pub mod binary;
pub mod commands;
pub mod discovery;
pub mod engine;
pub mod errors;
pub mod hunk;
pub mod parser;
pub(crate) mod untracked;
