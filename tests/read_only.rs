//! Read-only and offline verification.
//!
//! 1. Byte-level fingerprints of every file (content, mode, mtime) under
//!    the fixture repositories, remote included, before and after running
//!    `diffkit inspect` in many states. A positive control proves the
//!    fingerprint notices the index rewrite that plain `git status` does.
//! 2. A logging `git` wrapper records every Git invocation the binary makes.
//! 3. A source audit confirms only the command layer starts processes.

mod common;

use common::{diffkit, stderr, Remote, Repo, TempDir};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

type Fingerprint = BTreeMap<PathBuf, (u64, Option<SystemTime>, Vec<u8>, String)>;

fn fingerprint(root: &Path) -> Fingerprint {
    fn walk(root: &Path, dir: &Path, out: &mut Fingerprint) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) as u64;
            #[cfg(not(unix))]
            let mode = meta.permissions().readonly() as u64;
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if meta.is_dir() {
                out.insert(rel, (mode, None, Vec::new(), "dir".into()));
                walk(root, &p, out);
            } else if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&p).unwrap();
                out.insert(rel, (mode, None, Vec::new(), target.display().to_string()));
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.insert(rel, (mode, meta.modified().ok(), bytes, "file".into()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn assert_same(before: &Fingerprint, after: &Fingerprint) {
    let changed: Vec<_> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .collect();
    assert!(changed.is_empty(), "modified by inspection: {changed:?}");
}

/// Sets an old mtime on a tracked file without changing its content, so
/// Git sees stale stat data and would like to refresh the index.
fn make_stat_dirty(path: &Path) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
}

#[test]
fn inspect_leaves_every_repository_file_untouched() {
    let r = Remote::new("ro-fingerprint");
    let w = &r.work;
    w.write("staged.txt", "s\n");
    w.write("partial.txt", "p\n");
    w.write("touched.txt", "same content\n");
    w.write("nested/dir/file.txt", "n\n");
    w.commit_all("files");
    w.git(&["push", "-q"]);
    w.empty_commit("ahead");
    w.write("staged.txt", "s2\n");
    w.write("partial.txt", "p2\n");
    w.git(&["add", "staged.txt", "partial.txt"]);
    w.write("partial.txt", "p3\n");
    w.write("untracked/new.txt", "u\n");
    make_stat_dirty(&w.path.join("touched.txt"));
    let wt = r.tmp.join("linked");
    w.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "linked",
        wt.to_str().unwrap(),
    ]);
    std::fs::write(wt.join("staged.txt"), "changed in worktree\n").unwrap();

    let conflicted = Repo::init(r.tmp.join("conflicted"));
    conflicted.write("f", "base\n");
    conflicted.commit_all("base");
    conflicted.git(&["switch", "-q", "-c", "side"]);
    conflicted.write("f", "side\n");
    conflicted.commit_all("side");
    conflicted.git(&["switch", "-q", "main"]);
    conflicted.write("f", "main\n");
    conflicted.commit_all("main");
    assert!(!conflicted.git_output(&["merge", "side"]).status.success());

    let unborn = Repo::init(r.tmp.join("unborn"));
    unborn.write("a", "a");
    unborn.git(&["add", "a"]);

    let before = fingerprint(r.tmp.path());
    let dirs = [
        w.path.clone(),
        w.path.join("nested/dir"),
        wt.clone(),
        conflicted.path.clone(),
        unborn.path.clone(),
    ];
    for _ in 0..3 {
        for d in &dirs {
            let out = diffkit(d, &["inspect"]);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{}: {}",
                d.display(),
                stderr(&out)
            );
        }
    }
    assert_same(&before, &fingerprint(r.tmp.path()));

    // Positive control: ordinary `git status` does rewrite the index here,
    // so the fingerprint is sensitive to the change Diffkit avoids.
    let index = w.path.join(".git/index");
    let index_before = std::fs::read(&index).unwrap();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&w.path)
        .arg("status")
        .env_remove("GIT_OPTIONAL_LOCKS")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_ne!(
        std::fs::read(&index).unwrap(),
        index_before,
        "control: plain git status refreshes the index"
    );
}

#[test]
fn partial_clone_inspection_never_fetches_missing_objects() {
    let tmp = TempDir::new("ro-partial-clone");
    let src = Repo::init(tmp.join("src"));
    let body: String = (1..=300).map(|i| format!("{i}\n")).collect();
    src.write("a.txt", &body);
    src.commit_all("a");
    src.git(&["config", "uploadpack.allowFilter", "true"]);
    src.git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
    let url = format!("file://{}", src.path.display());
    common::git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--filter=blob:none",
            "--no-checkout",
            &url,
            "partial",
        ],
    );
    let partial = Repo {
        path: tmp.join("partial"),
    };
    // An inexact rename forces Git to read a blob that is only on the
    // promisor remote.
    partial.write("x.txt", &format!("{body}extra\n"));
    partial.git(&["add", "x.txt"]);
    let objects = || partial.git(&["count-objects", "-v"]);
    let before = objects();

    let out = diffkit(&partial.path, &["inspect"]);
    assert_eq!(objects(), before, "no objects were fetched");
    // Git refuses to fetch and fails; Diffkit reports that instead of
    // going to the network.
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("promisor remote"), "{}", stderr(&out));

    // Control: the same status without the guard does fetch.
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&partial.path)
        .args(["status", "--porcelain=v2"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_NO_LAZY_FETCH")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_ne!(
        objects(),
        before,
        "control: unguarded status lazily fetches"
    );
}

#[cfg(unix)]
mod audit {
    use super::*;
    use diffkit::git::status::STATUS_ARGS;

    const ALLOWED: &[&str] = &["rev-parse", "status", "for-each-ref"];

    struct Call {
        env: String,
        args: Vec<String>,
    }

    fn real_git() -> PathBuf {
        std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|d| d.join("git"))
            .find(|p| p.is_file())
            .expect("git on PATH")
    }

    struct Wrapper {
        bin: PathBuf,
        log: PathBuf,
    }

    impl Wrapper {
        fn new(tmp: &TempDir) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let bin = tmp.join("audit-bin");
            std::fs::create_dir_all(&bin).unwrap();
            let log = tmp.join("git-calls.log");
            let script = format!(
                "#!/bin/sh\n\
                 {{ printf 'LOCKS=%s LAZY=%s' \"${{GIT_OPTIONAL_LOCKS-unset}}\" \"${{GIT_NO_LAZY_FETCH-unset}}\"\n\
                 for a in \"$@\"; do printf '\\037%s' \"$a\"; done\n\
                 printf '\\n'; }} >> '{}'\n\
                 exec '{}' \"$@\"\n",
                log.display(),
                real_git().display()
            );
            let path = bin.join("git");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { bin, log }
        }

        /// Runs `diffkit inspect` with only the wrapper on PATH and returns
        /// the Git calls it made.
        fn run(&self, cwd: &Path) -> (i32, Vec<Call>) {
            let _ = std::fs::remove_file(&self.log);
            let out = std::process::Command::new(env!("CARGO_BIN_EXE_diffkit"))
                .arg("inspect")
                .current_dir(cwd)
                .env("PATH", &self.bin)
                .output()
                .unwrap();
            let log = std::fs::read_to_string(&self.log).unwrap_or_default();
            let calls = log
                .lines()
                .map(|l| {
                    let mut parts = l.split('\u{1f}');
                    Call {
                        env: parts.next().unwrap().to_owned(),
                        args: parts.map(str::to_owned).collect(),
                    }
                })
                .collect();
            (out.status.code().unwrap(), calls)
        }
    }

    fn check(calls: &[Call], cwd_hint: &str) -> Vec<String> {
        calls
            .iter()
            .map(|c| {
                assert_eq!(c.env, "LOCKS=0 LAZY=1", "{cwd_hint}: {:?}", c.args);
                assert_eq!(c.args[0], "-C", "{cwd_hint}: target directory is pinned");
                let sub = c.args[2].clone();
                assert!(
                    ALLOWED.contains(&sub.as_str()),
                    "{cwd_hint}: git {sub} is not allowed"
                );
                if sub == "status" {
                    assert_eq!(c.args[3..], STATUS_ARGS[1..], "{cwd_hint}");
                }
                sub
            })
            .collect()
    }

    #[test]
    fn every_git_invocation_is_an_allowed_read_only_command() {
        let r = Remote::new("ro-audit");
        let wrapper = Wrapper::new(&r.tmp);
        let empty = Repo::init(r.tmp.join("empty"));
        let plain = r.tmp.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let wt = r.tmp.join("wt");
        r.work
            .git(&["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);
        r.work.write("dirty.txt", "x");

        let mut scenarios: Vec<(&str, PathBuf, i32, Vec<&str>)> = vec![
            (
                "upstream available",
                r.work.path.clone(),
                0,
                vec!["rev-parse", "status", "rev-parse"],
            ),
            (
                "no upstream (worktree branch)",
                wt.clone(),
                0,
                vec!["rev-parse", "status"],
            ),
            (
                "empty repository",
                empty.path.clone(),
                0,
                vec!["rev-parse", "status", "for-each-ref"],
            ),
            ("not a repository", plain.clone(), 1, vec!["rev-parse"]),
        ];
        for (name, dir, code, expected) in scenarios.drain(..) {
            let (got_code, calls) = wrapper.run(&dir);
            assert_eq!(got_code, code, "{name}");
            assert_eq!(check(&calls, name), expected, "{name}");
            eprintln!("{name}: {} git subprocesses", calls.len());
        }

        r.work
            .git(&["config", "branch.main.merge", "refs/heads/missing"]);
        let (_, calls) = wrapper.run(&r.work.path);
        assert_eq!(check(&calls, "missing upstream"), ["rev-parse", "status"]);

        r.work
            .git(&["config", "branch.main.merge", "refs/heads/main"]);
        r.work.git(&["checkout", "-q", "--detach"]);
        let (_, calls) = wrapper.run(&r.work.path);
        assert_eq!(check(&calls, "detached"), ["rev-parse", "status"]);

        r.work.git(&["switch", "-q", "--orphan", "fresh"]);
        r.work.git(&["config", "branch.fresh.remote", "origin"]);
        r.work
            .git(&["config", "branch.fresh.merge", "refs/heads/main"]);
        let (_, calls) = wrapper.run(&r.work.path);
        assert_eq!(
            check(&calls, "unborn with upstream"),
            ["rev-parse", "status", "for-each-ref", "rev-parse"]
        );
    }
}

#[test]
fn only_the_command_layer_starts_processes() {
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let rel = f.strip_prefix(&src).unwrap();
        let spawns = text.matches("Command::new(").count();
        if rel == Path::new("git/commands.rs") {
            assert_eq!(spawns, 1, "exactly one spawn site");
            assert!(
                text.contains("Command::new(GIT)") && text.contains("const GIT: &str = \"git\";")
            );
        } else {
            assert_eq!(spawns, 0, "{} starts a process", rel.display());
        }
        for shell in ["\"sh\"", "\"bash\"", "\"cmd\"", "\"powershell\""] {
            assert!(!text.contains(shell), "{} mentions {shell}", rel.display());
        }
    }
}
