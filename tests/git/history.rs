//! Local commit inspection: metadata, commit diffs of every change kind,
//! recent history with a limit, shallow clones, invalid revisions, and the
//! `diffkit show` command. Checked against Git itself, read-only.

use crate::support;

use diffkit::{
    git::{errors::GitError, history},
    types::diff::{BinaryStatus, DiffContent, FileChangeKind},
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};
use support::{assert_read_only, engine, entry, git, Repo, TempDir};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

fn shallow_file(repo: &Repo) -> PathBuf {
    repo.path.join(".git/shallow")
}

/// Commits everything with explicit author and committer identities and
/// dates, and a message taken verbatim from a file.
fn commit_as(repo: &Repo, message: &str) -> String {
    let msg = repo.path.join(".git/TEST_MSG");
    std::fs::write(&msg, message).unwrap();
    repo.git(&["add", "-A"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(&repo.path)
        .args(["commit", "-q", "--allow-empty", "--cleanup=verbatim", "-F"])
        .arg(&msg)
        .env("GIT_AUTHOR_NAME", "Zoë Ångström")
        .env("GIT_AUTHOR_EMAIL", "zoe@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-02T03:04:05+05:30")
        .env("GIT_COMMITTER_NAME", "Commit Bot")
        .env("GIT_COMMITTER_EMAIL", "bot@example.invalid")
        .env("GIT_COMMITTER_DATE", "2026-02-03T04:05:06-08:00")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_file(msg).unwrap();
    repo.head()
}

#[test]
fn commit_metadata_matches_git() {
    let (_t, repo) = repo("hist-meta");
    repo.write("a.txt", "a\n");
    let parent = repo.commit_all("first");
    let id = commit_as(&repo, "Subject with ünïcode\n\nBody line 1\nBody line 2\n");
    let oid = history::resolve_commit(&repo.path, OsStr::new("HEAD")).unwrap();
    assert_eq!(oid.as_str(), id);
    let c = assert_read_only(&repo.path, || {
        history::commit(&repo.path, &oid, &shallow_file(&repo)).unwrap()
    });
    assert_eq!(c.id.as_str(), id);
    assert_eq!(
        c.short_id,
        repo.git(&["rev-parse", "--short", "HEAD"]).trim()
    );
    assert_eq!(
        c.parents.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
        vec![parent]
    );
    assert_eq!(c.author.name, "Zoë Ångström");
    assert_eq!(c.author.email, "zoe@example.invalid");
    assert_eq!(c.author.date, "2026-01-02T03:04:05+05:30");
    assert_eq!(c.committer.name, "Commit Bot");
    assert_eq!(c.committer.date, "2026-02-03T04:05:06-08:00");
    assert_eq!(c.subject(), "Subject with ünïcode");
    assert_eq!(
        c.message,
        "Subject with ünïcode\n\nBody line 1\nBody line 2"
    );
    assert!(!c.is_merge() && !c.shallow_boundary);
}

#[test]
fn commit_diff_covers_every_change_kind() {
    let (_t, repo) = repo("hist-kinds");
    repo.write("modified.txt", "one\ntwo words\n");
    repo.write("deleted.txt", "bye\n");
    repo.write("renamed-from.txt", &support::numbered("line ", 20));
    repo.write_bytes("image.bin", b"\x00\x01\x02");
    repo.write("run.sh", "echo\n");
    repo.commit_all("base");
    repo.write("added.txt", "new\n");
    repo.write("modified.txt", "one\ntwo words!\n");
    repo.remove("deleted.txt");
    repo.git(&["mv", "renamed-from.txt", "renamed-to.txt"]);
    repo.write_bytes("image.bin", b"\x00\x01\x02\x03\x04");
    let fake = "1234567890123456789012345678901234567890";
    repo.git(&["add", "-A"]);
    repo.git(&["update-index", "--chmod=+x", "run.sh"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{fake},vendor/lib"),
    ]);
    repo.git(&["commit", "-q", "-m", "every kind"]);
    let e = engine(&repo.path);
    let head = history::resolve_commit(&repo.path, OsStr::new("HEAD")).unwrap();
    let c = history::commit(&repo.path, &head, &shallow_file(&repo)).unwrap();
    assert_read_only(&repo.path, || {
        let list = e.discover_commit(&c.id, c.first_parent()).unwrap();
        assert_eq!(list.target.as_ref(), Some(&c.id));
        assert_eq!(list.base.as_ref(), c.first_parent());
        let kind = |p: &str| entry(&list, p).kind;
        assert_eq!(kind("added.txt"), FileChangeKind::Added);
        assert_eq!(kind("modified.txt"), FileChangeKind::Modified);
        assert_eq!(kind("deleted.txt"), FileChangeKind::Deleted);
        assert_eq!(kind("renamed-to.txt"), FileChangeKind::Renamed);
        assert_eq!(kind("run.sh"), FileChangeKind::ModeChanged);
        assert_eq!(kind("vendor/lib"), FileChangeKind::Added);
        assert_eq!(entry(&list, "image.bin").binary, BinaryStatus::Binary);
        let load = |p: &str| e.load_file_diff(&list, entry(&list, p).id).unwrap();
        let d = load("modified.txt");
        assert_eq!(
            support::rows(&d)[1],
            support::row(
                diffkit::types::diff::DiffRowKind::Modified,
                Some(2),
                Some(2),
                "two words",
                "two words!"
            )
        );
        match load("image.bin").content {
            DiffContent::Binary(b) => assert_eq!((b.old_size, b.new_size), (Some(3), Some(5))),
            other => panic!("{other:?}"),
        }
        match load("vendor/lib").content {
            DiffContent::Submodule(s) => {
                assert_eq!(s.new_commit.unwrap().as_str(), fake);
                assert_eq!(s.worktree, None, "a commit's submodule has no working tree");
            }
            other => panic!("{other:?}"),
        }
        assert!(load("renamed-to.txt").hunks().is_empty());
        assert!(load("run.sh").hunks().is_empty());
    });
}

#[test]
fn root_and_merge_commits() {
    let (_t, repo) = repo("hist-root-merge");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let root = repo.commit_all("root");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("side.txt", "s\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("main.txt", "m\n");
    repo.commit_all("main");
    repo.git(&["merge", "-q", "--no-ff", "--no-edit", "side"]);
    let e = engine(&repo.path);
    let h = history::recent(&repo.path, &shallow_file(&repo), 10).unwrap();
    assert_eq!(h.commits.len(), 4);
    let merge = &h.commits[0];
    assert!(merge.is_merge());
    let list = e.discover_commit(&merge.id, merge.first_parent()).unwrap();
    let paths: Vec<_> = list.files.iter().map(|f| f.path().to_owned()).collect();
    assert_eq!(
        paths,
        vec![Path::new("side.txt").to_path_buf()],
        "first parent only"
    );
    let root = h.commits.iter().find(|c| c.id.as_str() == root).unwrap();
    assert!(root.parents.is_empty() && !root.shallow_boundary);
    let list = e.discover_commit(&root.id, None).unwrap();
    assert!(list.files.iter().all(|f| f.kind == FileChangeKind::Added));
    assert_eq!(list.len(), 2);
}

#[test]
fn recent_history_is_bounded_newest_first_and_works_detached() {
    let (_t, repo) = repo("hist-recent");
    let mut ids = Vec::new();
    for i in 0..7 {
        repo.write("n.txt", &format!("{i}\n"));
        ids.push(repo.commit_all(&format!("commit {i}")));
    }
    let h = assert_read_only(&repo.path, || {
        history::recent(&repo.path, &shallow_file(&repo), 5).unwrap()
    });
    assert!(h.truncated);
    let got: Vec<_> = h.commits.iter().map(|c| c.id.to_string()).collect();
    let want: Vec<_> = ids.iter().rev().take(5).cloned().collect();
    assert_eq!(got, want);
    assert_eq!(h.commits[0].subject(), "commit 6");
    let all = history::recent(&repo.path, &shallow_file(&repo), 50).unwrap();
    assert!(!all.truncated && all.commits.len() == 7);
    repo.git(&["checkout", "-q", "--detach", &ids[3]]);
    let detached = history::recent(&repo.path, &shallow_file(&repo), 50).unwrap();
    assert_eq!(detached.commits.len(), 4);
    assert_eq!(detached.commits[0].id.as_str(), ids[3]);
}

#[test]
fn shallow_clone_marks_its_boundary() {
    let (t, origin) = repo("hist-shallow");
    for i in 0..5 {
        origin.write("n.txt", &format!("{i}\n"));
        origin.commit_all(&format!("c{i}"));
    }
    let url = format!("file://{}", origin.path.display());
    git(t.path(), &["clone", "-q", "--depth", "2", &url, "shallow"]);
    let clone = Repo {
        path: support::real(&t.join("shallow")),
    };
    assert!(clone.snapshot().repository.shallow);
    let h = history::recent(&clone.path, &shallow_file(&clone), 50).unwrap();
    assert_eq!(h.commits.len(), 2, "only local history");
    assert!(!h.truncated);
    let boundary = &h.commits[1];
    assert!(boundary.shallow_boundary && boundary.parents.is_empty());
    assert!(!h.commits[0].shallow_boundary);
}

#[test]
fn invalid_revisions_are_reported_not_misread() {
    let (_t, repo) = repo("hist-invalid");
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    repo.git(&["tag", "v1"]);
    let tree = repo.rev("HEAD^{tree}");
    for bad in [
        "",
        "zzzz",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "-h",
        "--all",
        "HEAD~99",
        "HEAD:a.txt",
        tree.as_str(),
        "main..v1",
    ] {
        match history::resolve_commit(&repo.path, OsStr::new(bad)) {
            Err(GitError::UnknownRevision { .. }) => {}
            other => panic!("{bad:?}: {other:?}"),
        }
    }
    let head = repo.head();
    for good in ["HEAD", "main", "v1", &head[..8]] {
        assert_eq!(
            history::resolve_commit(&repo.path, OsStr::new(good))
                .unwrap()
                .as_str(),
            head,
            "{good}"
        );
    }
    let (_u, unborn) = self::repo("hist-unborn");
    assert!(history::resolve_commit(&unborn.path, OsStr::new("HEAD")).is_err());
}

#[test]
fn show_command_prints_metadata_and_files() {
    let (_t, repo) = repo("hist-cli");
    repo.write("a.txt", "a\n");
    repo.commit_all("first");
    repo.write("a.txt", "a\nb\n");
    repo.write("new.txt", "n\n");
    let id = commit_as(&repo, "Second \u{1b}[31mred\u{1b}[0m commit\n\nbody\n");
    let out = assert_read_only(&repo.path, || {
        support::diffkit(&repo.path, &["show", "HEAD"])
    });
    assert!(out.status.success(), "{}", support::stderr(&out));
    let text = support::stdout(&out);
    assert!(text.starts_with("DIFFKIT - COMMIT\n"), "{text}");
    assert!(text.contains(&format!("Commit: {id}\n")));
    assert!(text.contains("Author: Zoë Ångström <zoe@example.invalid> 2026-01-02T03:04:05+05:30\n"));
    assert!(
        text.contains("    Second \\u{1b}[31mred\\u{1b}[0m commit\n"),
        "control characters are escaped: {text}"
    );
    assert!(!text.contains('\u{1b}'));
    assert!(
        text.contains("\nCOMMIT (parent -> commit): 2 files, +2 -0, 0 binary\n"),
        "{text}"
    );
    assert!(text.contains("  M   a.txt  +1 -0\n") && text.contains("  A   new.txt  +1 -0\n"));

    let missing = support::diffkit(&repo.path, &["show"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(support::stderr(&missing).contains("show needs a commit"));
    let extra = support::diffkit(&repo.path, &["show", "HEAD", "x"]);
    assert_eq!(extra.status.code(), Some(2));
    let bad = support::diffkit(&repo.path, &["show", "no-such-ref\u{1b}[2J"]);
    assert_eq!(bad.status.code(), Some(1));
    let err = support::stderr(&bad);
    assert!(
        err.contains("\"no-such-ref\\u{1b}[2J\" is not a commit"),
        "{err}"
    );
    assert!(!err.contains('\u{1b}'));
}

/// Every Git process `diffkit show` starts is read-only, recorded by a
/// logging `git` wrapper placed first on `PATH`.
#[cfg(unix)]
#[test]
fn show_runs_only_read_only_git_commands() {
    use std::os::unix::fs::PermissionsExt;
    let (t, repo) = repo("hist-audit");
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    repo.write("a.txt", "a\nb\n");
    repo.commit_all("two");
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    let bin = t.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = t.join("calls.log");
    std::fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\nprintf '%s %s\\n' \"$3\" \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = assert_read_only(&repo.path, || {
        support::isolate();
        Command::new(env!("CARGO_BIN_EXE_diffkit"))
            .args(["show", "HEAD~1"])
            .current_dir(&repo.path)
            .env("PATH", path)
            .output()
            .unwrap()
    });
    assert!(out.status.success(), "{}", support::stderr(&out));
    let calls = std::fs::read_to_string(&log).unwrap();
    let allowed = ["rev-parse", "status", "for-each-ref", "log", "diff-tree"];
    for line in calls.lines() {
        let (sub, all) = line.split_once(' ').unwrap();
        assert!(allowed.contains(&sub), "git {sub} is not allowed: {line}");
        if sub == "log" {
            assert!(all.contains("--no-show-signature") && all.contains("--end-of-options"));
        }
        if sub == "diff-tree" {
            assert!(all.contains("--no-ext-diff") && all.contains("--no-textconv"));
        }
    }
    for needed in ["log", "diff-tree", "rev-parse"] {
        assert!(
            calls.lines().any(|l| l.starts_with(needed)),
            "{needed}: {calls}"
        );
    }
}
