//! Argument handling, dispatch, and exit codes.
//!
//! Exit codes: 0 success, 1 inspection failed, 2 usage error.

use super::{diff_stats, report, show};
use diffkit::{
    diff::{DiffEngine, DiffError, DiffOptions},
    display::escape_text,
    git::snapshot,
};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::Path,
};

fn usage() -> String {
    format!(
        "diffkit {}\n\
         Read-only Git repository inspector.\n\n\
         Usage:\n  \
         diffkit [--no-watch] [PATH]\n                      \
         Open the interactive viewer for the repository containing\n                      \
         PATH (default: the current directory); press ? inside for keys.\n                      \
         It refreshes automatically when files or Git state change;\n                      \
         --no-watch turns that off (r still refreshes)\n  \
         diffkit inspect     Print a report for the repository containing the current directory\n  \
         diffkit diff-stats [--load]\n                      \
         List changed files per comparison (staged, unstaged,\n                      \
         combined, untracked); --load also parses every diff\n  \
         diffkit show <COMMIT>\n                      \
         Print a local commit's metadata and the files it changed\n                      \
         (a hash, branch, tag, or expression such as HEAD~2)\n  \
         diffkit --help      Show this help\n  \
         diffkit --version   Show the version\n\n\
         Subcommand names take priority over directory names; use ./inspect for a\n\
         directory called inspect.\n\n\
         Diffkit never modifies the repository and never contacts a remote.\n",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn run(args: Vec<OsString>) -> i32 {
    let text: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
    let text: Vec<&str> = text.iter().map(|a| a.as_ref()).collect();
    match text.as_slice() {
        [] | ["--no-watch"] => match std::env::current_dir() {
            Ok(dir) => tui(&dir, text.is_empty()),
            Err(e) => {
                fail(&format!("cannot read the current directory: {e}"));
                1
            }
        },
        ["--help" | "-h" | "help"] => print(&usage()),
        ["--version" | "-V"] => print(&format!("diffkit {}\n", env!("CARGO_PKG_VERSION"))),
        ["inspect"] => inspect(),
        ["inspect", extra, ..] => usage_error(&format!("unexpected argument for inspect: {extra}")),
        ["diff-stats"] => diff_stats(false),
        ["diff-stats", "--load"] => diff_stats(true),
        ["diff-stats", extra, ..] => {
            usage_error(&format!("unexpected argument for diff-stats: {extra}"))
        }
        ["show"] => usage_error("show needs a commit, for example `diffkit show HEAD`"),
        ["show", _] => show_commit(&args[1]),
        ["show", _, extra, ..] => usage_error(&format!("unexpected argument for show: {extra}")),
        [other] if !other.starts_with('-') => tui_at(&args[0], other, true),
        ["--no-watch", other] if !other.starts_with('-') => tui_at(&args[1], other, false),
        [other, ..] => usage_error(&format!("unknown command or option: {other}")),
    }
}

/// Opens the viewer at a path given on the command line. Uses the original
/// `OsString`: the path may not be UTF-8.
fn tui_at(arg: &OsString, shown: &str, watch: bool) -> i32 {
    let path = Path::new(arg);
    if path.is_dir() {
        tui(path, watch)
    } else {
        usage_error(&format!("not a command or a directory: {shown}"))
    }
}

fn tui(start: &Path, watch: bool) -> i32 {
    match diffkit::tui::run(start, diffkit::tui::RunOptions { watch }) {
        Ok(()) => 0,
        // The terminal is already restored; exit the way a shell reports
        // a process ended by that signal.
        Err(diffkit::tui::TuiError::Signal(n)) => 128 + n,
        Err(e) => {
            fail(&e);
            1
        }
    }
}

fn inspect() -> i32 {
    match snapshot::inspect_current_dir() {
        Ok(snapshot) => print(&report::render(&snapshot)),
        Err(e) => {
            fail(&e);
            1
        }
    }
}

fn diff_stats(load: bool) -> i32 {
    let result = snapshot::inspect_current_dir()
        .map_err(DiffError::from)
        .map(|s| DiffEngine::from_snapshot(&s, DiffOptions::default()))
        .and_then(|engine| diff_stats::render(&engine, load));
    match result {
        Ok(text) => print(&text),
        Err(e) => {
            fail(&e);
            1
        }
    }
}

/// Uses the original `OsString`: a ref name may not be UTF-8.
fn show_commit(rev: &OsString) -> i32 {
    let result = snapshot::inspect_current_dir()
        .map_err(DiffError::from)
        .and_then(|s| show::render(&s, rev));
    match result {
        Ok(text) => print(&text),
        Err(e) => {
            fail(&e);
            1
        }
    }
}

/// Prints an error. Messages can contain paths, revisions, and Git output,
/// so control characters are escaped before they reach the terminal.
fn fail(e: &dyn std::fmt::Display) {
    eprintln!("diffkit: error: {}", escape_text(&e.to_string()));
}

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
