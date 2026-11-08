//! Environmental and parse failures produce structured errors and useful
//! messages, never panics. Expected states are not errors.

use crate::support;

use diffkit::git::{errors::GitError, repository, snapshot, status};
use std::process::Command;
use support::{diffkit, stderr, stdout, Remote, Repo, TempDir};

#[test]
fn not_a_git_repository() {
    let tmp = TempDir::new("err-not-repo");
    let plain = tmp.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert!(matches!(
        snapshot::inspect(&plain),
        Err(GitError::NotARepository { .. })
    ));
    let out = diffkit(&plain, &["inspect"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("diffkit: error: "), "{err}");
    assert!(err.contains("is not inside a Git repository"), "{err}");
    assert!(!err.contains("panicked"));
}

#[test]
fn invalid_git_directory_pointer() {
    let tmp = TempDir::new("err-bad-gitfile");
    let dir = tmp.join("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".git"), "gitdir: /nonexistent/diffkit/worktree\n").unwrap();
    let e = repository::discover_from(&dir).unwrap_err();
    assert!(matches!(e, GitError::NotARepository { .. }), "{e:?}");
    let out = diffkit(&dir, &["inspect"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("not a git repository"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn corrupted_index_reports_git_failure_with_stderr() {
    let tmp = TempDir::new("err-corrupt-index");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a", "a");
    repo.commit_all("a");
    std::fs::write(repo.path.join(".git/index"), "garbage").unwrap();
    match snapshot::inspect(&repo.path) {
        Err(GitError::CommandFailed { args, code, stderr }) => {
            assert_eq!(args[0], "status");
            assert_eq!(code, Some(128));
            assert!(stderr.contains("index"), "{stderr}");
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
    let out = diffkit(&repo.path, &["inspect"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("`git status --porcelain=v2"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn corrupted_head_is_not_recognized_as_a_repository() {
    let tmp = TempDir::new("err-corrupt-head");
    let repo = Repo::init(tmp.join("repo"));
    std::fs::write(repo.path.join(".git/HEAD"), "not a ref\n").unwrap();
    let out = diffkit(&repo.path, &["inspect"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr(&out).contains("panicked"));
    assert!(snapshot::inspect(&repo.path).is_err());
}

#[test]
fn git_executable_not_on_path() {
    let tmp = TempDir::new("err-no-git");
    let repo = Repo::init(tmp.join("repo"));
    let empty_bin = tmp.join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_diffkit"))
        .arg("inspect")
        .current_dir(&repo.path)
        .env("PATH", &empty_bin)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("the `git` executable was not found"),
        "{}",
        stderr(&out)
    );
}

#[cfg(unix)]
#[test]
fn git_executable_without_execute_permission() {
    let tmp = TempDir::new("err-git-noexec");
    let repo = Repo::init(tmp.join("repo"));
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("git"), "not executable").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_diffkit"))
        .arg("inspect")
        .current_dir(&repo.path)
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("could not be run") || err.contains("was not found"),
        "{err}"
    );
    assert!(!err.contains("panicked"));
}

#[cfg(unix)]
#[test]
fn inaccessible_paths() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe_is_root() {
        eprintln!("SKIPPED: permission checks do not apply to root");
        return;
    }
    let tmp = TempDir::new("err-permissions");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("tracked.txt", "t");
    repo.commit_all("base");
    repo.write("locked/secret.txt", "s");
    let locked = repo.path.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let inside = snapshot::inspect(&locked.join("deeper"));
    let from_locked = repository::discover_from(&locked);
    let whole = snapshot::inspect(&repo.path);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        matches!(inside, Err(GitError::PathInaccessible { .. })),
        "{inside:?}"
    );
    match from_locked {
        Err(GitError::CommandFailed { stderr, .. }) => {
            assert!(stderr.contains("Permission denied"), "{stderr}")
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
    // Git skips the unreadable directory with a warning; inspection of the
    // rest of the repository still succeeds.
    let whole = whole.expect("unreadable subdirectory does not break inspection");
    assert!(whole
        .status
        .files
        .iter()
        .all(|f| !f.path.starts_with("locked/secret.txt")));
}

#[cfg(unix)]
fn unsafe_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[test]
fn nonexistent_directory() {
    let tmp = TempDir::new("err-missing-dir");
    match snapshot::inspect(&tmp.join("missing")) {
        Err(e @ GitError::PathInaccessible { .. }) => {
            assert!(e.to_string().starts_with("cannot access "), "{e}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn malformed_git_output_is_a_parse_error() {
    for raw in [
        &b"1 M. N... truncated\0"[..],
        b"2 R. N... 100644 100644 100644 a b R100 x\0",
        b"? missing-nul",
    ] {
        match status::parse(raw) {
            Err(e @ GitError::MalformedOutput { .. }) => {
                assert!(e.to_string().starts_with("unexpected Git output"))
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn expected_states_are_not_errors() {
    let tmp = TempDir::new("err-expected-states");
    let empty = Repo::init(tmp.join("empty"));
    let out = diffkit(&empty.path, &["inspect"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("empty repository"));

    let r = Remote::new("err-missing-upstream");
    r.work
        .git(&["config", "branch.main.merge", "refs/heads/missing"]);
    let out = diffkit(&r.work.path, &["inspect"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).is_empty());

    r.work.git(&["checkout", "-q", "--detach"]);
    assert_eq!(diffkit(&r.work.path, &["inspect"]).status.code(), Some(0));
}
