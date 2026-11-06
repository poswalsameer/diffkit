//! Phase 3 interactive terminal interface (Ratatui + Crossterm).
//!
//! - [`app`]: state and updates; the only part that calls the engines.
//! - [`actions`] / [`keymap`]: user intentions and the keys that trigger them.
//! - [`diff_view`]: display rows computed once per loaded diff.
//! - [`layout`]: pure screen geometry.
//! - [`ui`]: rendering from state only.
//! - [`terminal`]: raw mode / alternate screen guard and the event loop.

pub mod actions;
pub mod app;
pub mod diff_view;
pub mod keymap;
pub mod layout;
pub mod terminal;
pub mod ui;

pub use actions::Action;
pub use app::App;
pub use terminal::{run, TuiError};
