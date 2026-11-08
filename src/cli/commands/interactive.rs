//! `diffkit [--no-watch] [PATH]`: the interactive viewer.

use super::super::{fail, usage_error};
use crate::tui::terminal::{self, RunOptions, TuiError};
use std::{ffi::OsStr, path::Path};

/// Opens the viewer for `path` (the current directory if `None`). A path
/// that is not a directory is a usage error: it was neither a command nor
/// a directory. The original `OsStr` is used; the path may not be UTF-8.
pub fn run(path: Option<&OsStr>, watch: bool) -> i32 {
    let start = match path {
        Some(arg) if Path::new(arg).is_dir() => Path::new(arg).to_path_buf(),
        Some(arg) => {
            return usage_error(&format!(
                "not a command or a directory: {}",
                arg.to_string_lossy()
            ))
        }
        None => match std::env::current_dir() {
            Ok(dir) => dir,
            Err(e) => {
                fail(&format!("cannot read the current directory: {e}"));
                return 1;
            }
        },
    };
    match terminal::run(&start, RunOptions { watch }) {
        Ok(()) => 0,
        // The terminal is already restored; exit the way a shell reports
        // a process ended by that signal.
        Err(TuiError::Signal(n)) => 128 + n,
        Err(e) => {
            fail(&e);
            1
        }
    }
}
