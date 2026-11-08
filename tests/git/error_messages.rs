//! Git error messages: actionable text and escaped untrusted input.

use diffkit::git::errors::*;
use std::path::PathBuf;

#[test]
fn command_failure_message_includes_args_code_and_stderr() {
    let e = GitError::CommandFailed {
        args: vec!["status".into(), "-z".into()],
        code: Some(128),
        stderr: "fatal: index file corrupt\n".into(),
    };
    assert_eq!(
        e.to_string(),
        "`git status -z` failed with exit code 128: fatal: index file corrupt"
    );
}

#[test]
fn not_a_repository_message_names_path() {
    let e = GitError::NotARepository {
        path: PathBuf::from("/tmp/x"),
        stderr: String::new(),
    };
    assert_eq!(e.to_string(), "/tmp/x is not inside a Git repository");
}

#[test]
fn git_not_found_is_actionable() {
    assert!(GitError::GitNotFound.to_string().contains("PATH"));
}
