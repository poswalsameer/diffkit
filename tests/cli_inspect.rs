//! Black-box tests of the `diffkit` binary's output.

mod common;

use common::{diffkit, stderr, stdout, Remote, Repo, TempDir};
use std::process::{Command, Stdio};

fn inspect_ok(dir: &std::path::Path) -> String {
    let out = diffkit(dir, &["inspect"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    stdout(&out)
}

/// Asserts `label:` is immediately followed by the given indented lines.
fn assert_field(out: &str, label: &str, lines: &[&str]) {
    let mut expected = format!("\n{label}:\n");
    for l in lines {
        expected.push_str(&format!("  {l}\n"));
    }
    expected.push('\n');
    assert!(out.contains(&expected), "missing {expected:?} in:\n{out}");
}

#[test]
fn help_version_and_default() {
    let tmp = TempDir::new("cli-help");
    for args in [&[][..], &["--help"], &["-h"]] {
        let out = diffkit(tmp.path(), args);
        assert_eq!(out.status.code(), Some(0));
        let text = stdout(&out);
        assert!(text.contains("Usage:"), "{text}");
        assert!(text.contains("diffkit inspect"));
    }
    for args in [&["--version"][..], &["-V"]] {
        let out = diffkit(tmp.path(), args);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            stdout(&out),
            format!("diffkit {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn usage_errors_exit_2() {
    let tmp = TempDir::new("cli-usage");
    for args in [&["frobnicate"][..], &["inspect", "extra"], &["--bogus"]] {
        let out = diffkit(tmp.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stderr(&out).contains("diffkit --help"));
        assert!(stdout(&out).is_empty());
    }
}

#[test]
fn clean_repository_with_upstream() {
    let r = Remote::new("cli-clean");
    let out = inspect_ok(&r.work.path);
    let root = r.work.path.display().to_string();
    let git_dir = r.work.path.join(".git").display().to_string();
    let head = r.work.head();
    assert!(out.starts_with(
        "------------------------------------------------------------\nDIFFKIT - REPOSITORY INSPECTOR\n"
    ));
    assert_field(&out, "Root", &[&root]);
    assert_field(&out, "Git directory", &[&git_dir]);
    assert_field(&out, "Common Git directory", &[&git_dir]);
    assert_field(&out, "Repository type", &["Standard repository"]);
    assert_field(&out, "Current branch", &["main"]);
    assert_field(&out, "HEAD", &[&head]);
    assert_field(&out, "Branch state", &["Normal branch"]);
    assert_field(
        &out,
        "Upstream",
        &["origin/main (refs/remotes/origin/main)"],
    );
    assert_field(&out, "Upstream commit", &[&head]);
    assert_field(
        &out,
        "Tracking status",
        &[
            "Up to date with the locally known upstream",
            "Ahead by 0 commits",
            "Behind by 0 commits",
        ],
    );
    assert_field(
        &out,
        "Information source",
        &[
            "Locally available remote-tracking reference.",
            "Remote state has not been verified by fetching; it may be stale.",
        ],
    );
    assert_field(
        &out,
        "State",
        &["Clean: no staged, unstaged, untracked, or conflicted files"],
    );
    assert!(out.contains("FILES\n\n  (none)\n"));
    assert!(out.ends_with(
        "END OF REPORT\n------------------------------------------------------------\n"
    ));
}

#[test]
fn representative_dirty_state() {
    let r = Remote::new("cli-dirty");
    let w = &r.work;
    for f in [
        "staged.txt",
        "partial.txt",
        "unstaged.txt",
        "deleted.txt",
        "old-name.txt",
    ] {
        w.write(
            f,
            &format!("{f}\nsome shared content to make renames detectable\n"),
        );
    }
    w.commit_all("files");
    w.git(&["push", "-q"]);
    w.empty_commit("local 1");
    w.empty_commit("local 2");
    r.publish_remote_commits(1);
    w.git(&["fetch", "-q"]);
    w.write("staged.txt", "staged change");
    w.write("partial.txt", "first");
    w.git(&["add", "staged.txt", "partial.txt"]);
    w.write("partial.txt", "second");
    w.write("unstaged.txt", "unstaged change");
    w.git(&["rm", "-q", "deleted.txt"]);
    w.git(&["mv", "old-name.txt", "new-name.txt"]);
    w.write("notes/todo.md", "untracked");

    let out = inspect_ok(&w.path);
    assert_field(
        &out,
        "Tracking status",
        &[
            "Diverged from the locally known upstream",
            "Ahead by 2 commits",
            "Behind by 1 commit",
        ],
    );
    assert_field(&out, "State", &["Changes present"]);
    for (label, n) in [
        ("Staged", "4 files"),
        ("Unstaged", "2 files"),
        ("Partially staged", "1 file"),
        ("Untracked", "1 file"),
        ("Conflicted", "0 files"),
        ("Deleted", "1 file"),
        ("Renamed", "1 file"),
        ("Copied", "0 files"),
    ] {
        assert_field(&out, label, &[n]);
    }
    let staged = "\nSTAGED (index differs from HEAD)\n";
    let section = &out[out.find(staged).expect(staged)..];
    let section = &section[..section[1..].find("\n\n").unwrap() + 1];
    for line in [
        "  D.  deleted.txt  (deleted)",
        "  R.  old-name.txt -> new-name.txt  (renamed)",
        "  MM  partial.txt  (modified)",
        "  M.  staged.txt  (modified)",
    ] {
        assert!(section.contains(line), "{line:?} in {section}");
    }
    assert!(out.contains(
        "\nUNSTAGED (working tree differs from index)\n  MM  partial.txt  (modified)\n  .M  unstaged.txt  (modified)\n"
    ), "{out}");
    assert!(out.contains(
        "\nPARTIALLY STAGED (both staged and unstaged changes)\n  MM  partial.txt  (staged modified, unstaged modified)\n"
    ));
    assert!(out.contains("\nUNTRACKED\n  ??  notes/todo.md  (untracked)\n"));
    assert!(out.contains("\nDELETED\n  D.  deleted.txt  (deletion staged)\n"));
    assert!(out.contains(
        "\nRENAMED\n  R.  old-name.txt -> new-name.txt  (renamed, staged, 100% similar)\n"
    ));
    assert!(!out.contains("\nCONFLICTED"));
}

#[test]
fn conflicts_are_listed() {
    let tmp = TempDir::new("cli-conflict");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("both.txt", "base\n");
    repo.write("gone.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("both.txt", "side\n");
    repo.remove("gone.txt");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("both.txt", "main\n");
    repo.write("gone.txt", "main\n");
    repo.commit_all("main");
    assert!(!repo.git_output(&["merge", "side"]).status.success());
    let out = inspect_ok(&repo.path);
    assert_field(&out, "Conflicted", &["2 files"]);
    assert!(out.contains(
        "\nCONFLICTED (unmerged)\n  UU  both.txt  (both modified)\n  UD  gone.txt  (deleted by them)\n"
    ), "{out}");
}

#[test]
fn detached_head_output() {
    let r = Remote::new("cli-detached");
    r.work.git(&["checkout", "-q", "--detach"]);
    let out = inspect_ok(&r.work.path);
    assert_field(&out, "Current branch", &["(none: HEAD is detached)"]);
    assert_field(&out, "Branch state", &["Detached HEAD"]);
    assert_field(
        &out,
        "Upstream",
        &["Not applicable: HEAD is not on a branch"],
    );
    assert_field(&out, "Tracking status", &["Unavailable: HEAD is detached"]);
}

#[test]
fn empty_repository_output() {
    let tmp = TempDir::new("cli-empty");
    let repo = Repo::init(tmp.join("repo"));
    let out = inspect_ok(&repo.path);
    assert_field(&out, "Current branch", &["main"]);
    assert_field(&out, "HEAD", &["(none: no commits yet)"]);
    assert_field(
        &out,
        "Branch state",
        &["Unborn branch: empty repository, no commits yet"],
    );
    assert_field(
        &out,
        "Upstream",
        &["No upstream configured for branch main"],
    );
}

#[test]
fn missing_upstream_output() {
    let r = Remote::new("cli-missing-upstream");
    r.work
        .git(&["config", "branch.main.merge", "refs/heads/vanished"]);
    let out = inspect_ok(&r.work.path);
    assert_field(
        &out,
        "Upstream",
        &["origin/vanished (configured, but this ref does not exist locally)"],
    );
    assert_field(&out, "Upstream commit", &["(unavailable)"]);
    assert_field(
        &out,
        "Tracking status",
        &["Unavailable: the upstream ref is missing locally (never fetched, or deleted and pruned)"],
    );
    assert!(!out.contains("Ahead by"));
}

#[test]
fn no_upstream_output() {
    let tmp = TempDir::new("cli-no-upstream");
    let repo = Repo::init(tmp.join("repo"));
    repo.empty_commit("c");
    let out = inspect_ok(&repo.path);
    assert_field(
        &out,
        "Upstream",
        &["No upstream configured for branch main"],
    );
    assert_field(
        &out,
        "Tracking status",
        &["Unavailable: no upstream is configured"],
    );
    assert!(!out.contains("Ahead by"), "no invented zero counts");
    assert!(!out.contains("Information source"));
}

#[test]
fn nested_directory_and_linked_worktree_output() {
    let tmp = TempDir::new("cli-worktree");
    let repo = Repo::init(tmp.join("main"));
    repo.empty_commit("c");
    let wt = tmp.join("linked tree");
    repo.git(&["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);
    std::fs::create_dir_all(wt.join("deep/er")).unwrap();
    let out = inspect_ok(&wt.join("deep/er"));
    assert_field(&out, "Root", &[&wt.display().to_string()]);
    assert_field(
        &out,
        "Current directory",
        &[&wt.join("deep/er").display().to_string()],
    );
    // Git sanitizes the worktree's admin directory name ("linked-tree").
    let git_dir = common::git(&wt, &["rev-parse", "--absolute-git-dir"]);
    assert!(git_dir
        .trim()
        .starts_with(repo.path.join(".git/worktrees/").to_str().unwrap()));
    assert_field(&out, "Git directory", &[git_dir.trim()]);
    assert_field(
        &out,
        "Common Git directory",
        &[&repo.path.join(".git").display().to_string()],
    );
    assert_field(
        &out,
        "Repository type",
        &["Linked worktree (its Git directory is inside the common Git directory)"],
    );
    assert_field(&out, "Current branch", &["wt"]);
}

#[test]
fn closed_stdout_is_not_a_crash() {
    let tmp = TempDir::new("cli-pipe");
    let repo = Repo::init(tmp.join("repo"));
    for i in 0..3000 {
        repo.write(format!("untracked-file-with-a-long-name-{i:05}.txt"), "x");
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_diffkit"))
        .arg("inspect")
        .current_dir(&repo.path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("panicked"));
}
