//! Diffkit: a read-only, terminal-native Git inspector.
//!
//! Features own their logic; data shared between them lives in [`types`],
//! and pure utilities shared between them in [`helpers`].
//!
//! - [`git`]: repository inspection through Git subprocesses.
//! - [`diff`]: change discovery, lazy diff loading, and alignment.
//! - [`sync`]: keeping the state current when the repository changes.
//! - [`tui`]: the interactive terminal interface.
//! - [`cli`]: command-line entry point and the plain-text commands.

pub mod cli;
pub mod diff;
pub mod git;
pub mod helpers;
pub mod sync;
pub mod tui;
pub mod types;
