//! Pure, synchronous utilities shared between features. Nothing here runs
//! Git, touches the file system, or knows about a feature's own types.
//!
//! - [`display`]: escaping, width, and truncation for anything shown.
//! - [`git_text`]: byte-level conventions of Git's text output.

pub mod display;
pub mod git_text;
