//! Command-line arguments: what each form means, without running anything.
//!
//! Subcommand names take priority over directory names. Arguments are kept
//! as `OsString` where they become paths or revisions, which may not be
//! UTF-8.

use std::ffi::OsString;

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The interactive viewer, for `path` or the current directory.
    Interactive {
        path: Option<OsString>,
        watch: bool,
    },
    Inspect,
    DiffStats {
        load: bool,
    },
    Show {
        rev: OsString,
    },
    Help,
    Version,
}

/// Parses the arguments after the program name. `Err` holds the usage
/// message for an invalid command line.
pub fn parse(args: &[OsString]) -> Result<Command, String> {
    let text: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
    let text: Vec<&str> = text.iter().map(|a| a.as_ref()).collect();
    match text.as_slice() {
        [] => Ok(Command::Interactive {
            path: None,
            watch: true,
        }),
        ["--no-watch"] => Ok(Command::Interactive {
            path: None,
            watch: false,
        }),
        ["--help" | "-h" | "help"] => Ok(Command::Help),
        ["--version" | "-V"] => Ok(Command::Version),
        ["inspect"] => Ok(Command::Inspect),
        ["inspect", extra, ..] => Err(format!("unexpected argument for inspect: {extra}")),
        ["diff-stats"] => Ok(Command::DiffStats { load: false }),
        ["diff-stats", "--load"] => Ok(Command::DiffStats { load: true }),
        ["diff-stats", extra, ..] => Err(format!("unexpected argument for diff-stats: {extra}")),
        ["show"] => Err("show needs a commit, for example `diffkit show HEAD`".into()),
        ["show", _] => Ok(Command::Show {
            rev: args[1].clone(),
        }),
        ["show", _, extra, ..] => Err(format!("unexpected argument for show: {extra}")),
        [other] if !other.starts_with('-') => Ok(Command::Interactive {
            path: Some(args[0].clone()),
            watch: true,
        }),
        ["--no-watch", other] if !other.starts_with('-') => Ok(Command::Interactive {
            path: Some(args[1].clone()),
            watch: false,
        }),
        [other, ..] => Err(format!("unknown command or option: {other}")),
    }
}

pub fn usage() -> String {
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
