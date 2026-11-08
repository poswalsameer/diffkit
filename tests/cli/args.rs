//! Command-line parsing: every accepted form, and the usage errors for the
//! rest. Paths and revisions are kept as given, including non-UTF-8 bytes.

use diffkit::cli::args::{parse, usage, Command};
use std::ffi::OsString;

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn every_accepted_form() {
    let interactive = |path: Option<&str>, watch| Command::Interactive {
        path: path.map(OsString::from),
        watch,
    };
    for (given, want) in [
        (&[][..], interactive(None, true)),
        (&["--no-watch"], interactive(None, false)),
        (&["some/dir"], interactive(Some("some/dir"), true)),
        (&["--no-watch", "dir"], interactive(Some("dir"), false)),
        (&["inspect"], Command::Inspect),
        (&["diff-stats"], Command::DiffStats { load: false }),
        (&["diff-stats", "--load"], Command::DiffStats { load: true }),
        (
            &["show", "HEAD~2"],
            Command::Show {
                rev: "HEAD~2".into(),
            },
        ),
        (&["--help"], Command::Help),
        (&["-h"], Command::Help),
        (&["help"], Command::Help),
        (&["--version"], Command::Version),
        (&["-V"], Command::Version),
    ] {
        assert_eq!(parse(&args(given)), Ok(want), "{given:?}");
    }
}

#[test]
fn invalid_forms_are_usage_errors() {
    for (given, message) in [
        (&["inspect", "x"][..], "unexpected argument for inspect: x"),
        (
            &["diff-stats", "--fast"],
            "unexpected argument for diff-stats: --fast",
        ),
        (&["show"], "show needs a commit"),
        (&["show", "a", "b"], "unexpected argument for show: b"),
        (&["--frobnicate"], "unknown command or option: --frobnicate"),
        (
            &["--no-watch", "--load"],
            "unknown command or option: --no-watch",
        ),
        (&["a", "b"], "unknown command or option: a"),
    ] {
        let err = parse(&args(given)).expect_err(&format!("{given:?}"));
        assert!(err.starts_with(message), "{given:?}: {err}");
    }
}

#[test]
fn subcommand_names_win_over_directories() {
    assert_eq!(parse(&args(&["inspect"])), Ok(Command::Inspect));
    assert_eq!(
        parse(&args(&["./inspect"])),
        Ok(Command::Interactive {
            path: Some("./inspect".into()),
            watch: true
        })
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_and_revisions_are_kept_byte_for_byte() {
    use std::os::unix::ffi::OsStringExt;
    let raw = OsString::from_vec(b"dir-\xff".to_vec());
    assert_eq!(
        parse(std::slice::from_ref(&raw)),
        Ok(Command::Interactive {
            path: Some(raw.clone()),
            watch: true
        })
    );
    assert_eq!(
        parse(&[OsString::from("show"), raw.clone()]),
        Ok(Command::Show { rev: raw })
    );
}

#[test]
fn usage_lists_every_command() {
    let text = usage();
    for command in [
        "inspect",
        "diff-stats [--load]",
        "show <COMMIT>",
        "--no-watch",
        "--help",
        "--version",
    ] {
        assert!(text.contains(command), "{command}");
    }
}
