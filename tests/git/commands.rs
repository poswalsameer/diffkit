//! The Git command layer: failure reporting, bounded output, invocation
//! counting, and lossless path conversion.

use diffkit::git::{commands::*, errors::GitError, paths::*};

#[cfg(unix)]
#[test]
fn unix_paths_keep_raw_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let p = path_from_git(b"raw-\xff\xfe name\n").unwrap();
    assert_eq!(p.as_os_str().as_bytes(), b"raw-\xff\xfe name\n");
}

#[cfg(windows)]
#[test]
fn windows_paths_decode_utf8_strictly() {
    assert_eq!(
        path_from_git("dir/caf\u{e9} \u{65e5}.txt".as_bytes()).unwrap(),
        std::path::PathBuf::from("dir/caf\u{e9} \u{65e5}.txt")
    );
    assert!(path_from_git(b"raw-\xff").is_err());
}

#[test]
fn failing_command_reports_stderr_and_code() {
    let dir = std::env::temp_dir();
    let err = run(
        &dir,
        &["rev-parse", "--verify", "refs/diffkit/definitely-missing"],
    )
    .expect_err("rev-parse of a missing ref must fail");
    match err {
        GitError::CommandFailed { args, code, .. } => {
            assert_eq!(args[0], "rev-parse");
            assert!(code.is_some());
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn limited_output_reports_overflow_instead_of_truncating() {
    let dir = std::env::temp_dir();
    match output_limited(&dir, &["--version"], 4).unwrap() {
        LimitedOutput::Exceeded { limit } => assert_eq!(limit, 4),
        LimitedOutput::Complete(_) => panic!("`git --version` is longer than 4 bytes"),
    }
    match output_limited(&dir, &["--version"], 1 << 20).unwrap() {
        LimitedOutput::Complete(out) => {
            assert!(out.status.success());
            assert!(out.stdout.starts_with(b"git version"));
        }
        LimitedOutput::Exceeded { .. } => panic!("unexpected overflow"),
    }
}

#[test]
fn invocations_are_counted_per_thread() {
    let before = thread_invocations();
    let _ = output(&std::env::temp_dir(), &["--version"]);
    assert_eq!(thread_invocations(), before + 1);
}
