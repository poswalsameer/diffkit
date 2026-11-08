//! Command-line entry point: parses the arguments ([`args`]), runs the
//! command ([`commands`]), and maps the outcome to an exit code.
//!
//! Exit codes: 0 success, 1 inspection failed, 2 usage error, 128 + N when
//! the interactive viewer was stopped by signal N.

pub mod args;
pub mod commands;

use crate::helpers::display::escape_text;
use args::Command;
use std::{
    ffi::OsString,
    io::{self, Write},
};

/// Runs Diffkit with the arguments after the program name and returns the
/// process exit code.
pub fn run(args: Vec<OsString>) -> i32 {
    let command = match args::parse(&args) {
        Ok(command) => command,
        Err(message) => return usage_error(&message),
    };
    match command {
        Command::Interactive { path, watch } => commands::interactive::run(path.as_deref(), watch),
        Command::Inspect => commands::inspect::run(),
        Command::DiffStats { load } => commands::diff_stats::run(load),
        Command::Show { rev } => commands::show::run(&rev),
        Command::Help => print(&args::usage()),
        Command::Version => print(&format!("diffkit {}\n", env!("CARGO_PKG_VERSION"))),
    }
}

/// Prints an error. Messages can contain paths, revisions, and Git output,
/// so control characters are escaped before they reach the terminal.
fn fail(e: &dyn std::fmt::Display) {
    eprintln!("diffkit: error: {}", escape_text(&e.to_string()));
}

/// Prints `message` with a pointer to `--help`; exit code 2.
fn usage_error(message: &str) -> i32 {
    eprintln!(
        "diffkit: {}\nRun `diffkit --help` for usage.",
        escape_text(message)
    );
    2
}

/// Writes to stdout; a closed pipe (`diffkit inspect | head`) is not an error.
fn print(text: &str) -> i32 {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            fail(&format!("cannot write output: {e}"));
            1
        }
    }
}

/// Prints a command's text, or its error with exit code 1.
fn finish(result: Result<String, impl std::fmt::Display>) -> i32 {
    match result {
        Ok(text) => print(&text),
        Err(e) => {
            fail(&e);
            1
        }
    }
}
