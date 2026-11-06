//! Argument handling, dispatch, and exit codes.
//!
//! Exit codes: 0 success, 1 inspection failed, 2 usage error.

use super::report;
use diffkit::git::snapshot;
use std::{
    ffi::OsString,
    io::{self, Write},
};

fn usage() -> String {
    format!(
        "diffkit {}\n\
         Read-only Git repository inspector.\n\n\
         Usage:\n  \
         diffkit inspect     Inspect the repository containing the current directory\n  \
         diffkit --help      Show this help\n  \
         diffkit --version   Show the version\n\n\
         Diffkit never modifies the repository and never contacts a remote.\n",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn run(args: Vec<OsString>) -> i32 {
    let args: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
    let args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
    match args.as_slice() {
        [] | ["--help" | "-h" | "help"] => print(&usage()),
        ["--version" | "-V"] => print(&format!("diffkit {}\n", env!("CARGO_PKG_VERSION"))),
        ["inspect"] => inspect(),
        ["inspect", extra, ..] => usage_error(&format!("unexpected argument for inspect: {extra}")),
        [other, ..] => usage_error(&format!("unknown command or option: {other}")),
    }
}

fn inspect() -> i32 {
    match snapshot::inspect_current_dir() {
        Ok(snapshot) => print(&report::render(&snapshot)),
        Err(e) => {
            eprintln!("diffkit: error: {e}");
            1
        }
    }
}

fn usage_error(message: &str) -> i32 {
    eprintln!("diffkit: {message}\nRun `diffkit --help` for usage.");
    2
}

/// Writes to stdout; a closed pipe (`diffkit inspect | head`) is not an error.
fn print(text: &str) -> i32 {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            eprintln!("diffkit: error: cannot write output: {e}");
            1
        }
    }
}
