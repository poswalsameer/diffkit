//! The diff engine never writes to the repository, never runs external
//! diff programs or textconv filters, and only runs allowed Git commands.

mod common;

use common::{diffkit, engine, stderr, Repo, TempDir};
#[cfg(unix)]
use common::{load, rows};
use diffkit::diff::DiffComparison;
#[cfg(unix)]
use diffkit::diff::DiffComparison::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

fn fingerprint(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, Option<SystemTime>, bool)> {
    fn walk(
        root: &Path,
        dir: &Path,
        out: &mut BTreeMap<PathBuf, (Vec<u8>, Option<SystemTime>, bool)>,
    ) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if m.is_dir() {
                out.insert(rel, (Vec::new(), None, true));
                walk(root, &p, out);
            } else if m.file_type().is_symlink() {
                out.insert(
                    rel,
                    (
                        std::fs::read_link(&p)
                            .unwrap()
                            .into_os_string()
                            .into_encoded_bytes(),
                        None,
                        false,
                    ),
                );
            } else {
                out.insert(rel, (std::fs::read(&p).unwrap(), m.modified().ok(), false));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn diff_engine_leaves_every_file_untouched() {
    let tmp = TempDir::new("diff-ro-fingerprint");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("staged.txt", "s\n");
    repo.write("partial.txt", "p\n");
    repo.write("touched.txt", "same\n");
    repo.write("c.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("c.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("c.txt", "main\n");
    repo.commit_all("main");
    assert!(!repo.git_output(&["merge", "side"]).status.success());
    repo.write("staged.txt", "s2\n");
    repo.write("partial.txt", "p2\n");
    repo.git(&["add", "staged.txt", "partial.txt"]);
    repo.write("partial.txt", "p3\n");
    repo.write("untracked/new.txt", "u\n");
    std::fs::File::options()
        .write(true)
        .open(repo.path.join("touched.txt"))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();

    let before = fingerprint(tmp.path());
    for _ in 0..2 {
        let out = diffkit(&repo.path, &["diff-stats", "--load"]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let e = engine(&repo.path);
        for c in DiffComparison::ALL {
            let list = e.discover(c).unwrap();
            for f in &list.files {
                let _ = e.load_file_diff(&list, f.id);
            }
            e.load_all(&list).unwrap();
        }
    }
    let after = fingerprint(tmp.path());
    let changed: Vec<_> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .collect();
    assert!(changed.is_empty(), "modified: {changed:?}");
}

#[cfg(unix)]
#[test]
fn external_diff_and_textconv_are_never_run() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("diff-ro-ext");
    let repo = Repo::init(tmp.join("repo"));
    let marker = tmp.join("marker");
    let script = tmp.join("tool.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho ran >> '{}'\ntr a-z A-Z < \"$1\"\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.write(".gitattributes", "*.txt diff=conv\n");
    repo.write("f.txt", "lower case\n");
    repo.commit_all("base");
    repo.git(&["config", "diff.conv.textconv", script.to_str().unwrap()]);
    repo.git(&["config", "diff.external", script.to_str().unwrap()]);
    repo.write("f.txt", "lower case changed\n");
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", "lower case changed again\n");

    let e = engine(&repo.path);
    for c in [Staged, Unstaged, Combined] {
        let d = load(&e, c, "f.txt");
        assert!(
            rows(&d)
                .iter()
                .all(|r| !r.3.contains("LOWER") && !r.4.contains("LOWER")),
            "{c:?}: textconv output"
        );
        e.load_all(&e.discover(c).unwrap()).unwrap();
    }
    assert_eq!(
        diffkit(&repo.path, &["diff-stats", "--load"]).status.code(),
        Some(0)
    );
    assert!(!marker.exists(), "an external program was executed");

    // Control: the configuration is live for an ordinary `git diff`.
    let out = repo.git_output(&["diff", "--cached"]);
    assert!(out.status.success());
    assert!(
        marker.exists(),
        "control: plain git diff runs the configured program"
    );
}

#[cfg(unix)]
#[test]
fn diff_stats_runs_only_allowed_git_commands() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("diff-ro-audit");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "a\n");
    std::fs::write(repo.path.join("b.bin"), b"\x00bin").unwrap();
    repo.commit_all("base");
    repo.write("a.txt", "A\n");
    repo.git(&["add", "a.txt"]);
    std::fs::write(repo.path.join("b.bin"), b"\x00bin changed").unwrap();
    repo.write("new.txt", "n\n");

    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = tmp.join("calls.log");
    std::fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\n{{ printf 'LITERAL=%s LOCKS=%s' \"$GIT_LITERAL_PATHSPECS\" \"$GIT_OPTIONAL_LOCKS\"; for a in \"$@\"; do printf '\\037%s' \"$a\"; done; printf '\\n'; }} >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_diffkit"))
        .args(["diff-stats", "--load"])
        .current_dir(&repo.path)
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // Porcelain `git diff` is deliberately absent: it rewrites .git/index.
    let allowed = [
        "rev-parse",
        "status",
        "for-each-ref",
        "diff-index",
        "diff-files",
        "ls-files",
        "cat-file",
    ];
    let calls = std::fs::read_to_string(&log).unwrap();
    let mut subs = Vec::new();
    for line in calls.lines() {
        let mut parts = line.split('\u{1f}');
        assert_eq!(parts.next().unwrap(), "LITERAL=1 LOCKS=0");
        let args: Vec<&str> = parts.collect();
        assert_eq!(args[0], "-C");
        let sub = args[2];
        assert!(allowed.contains(&sub), "git {sub} is not allowed");
        if sub.starts_with("diff-") {
            for flag in ["--no-ext-diff", "--no-textconv", "--no-color"] {
                assert!(args.contains(&flag), "{flag} missing in {args:?}");
            }
        }
        subs.push(sub.to_owned());
    }
    // rev-parse + status (snapshot), then per comparison: staged/unstaged/
    // combined = discover + load_all, untracked = ls-files; the binary
    // file's sizes add cat-file calls.
    assert_eq!(
        subs.iter().filter(|s| s.starts_with("diff-")).count(),
        6,
        "{subs:?}"
    );
    assert_eq!(subs.iter().filter(|s| *s == "ls-files").count(), 1);
    assert_eq!(subs.iter().filter(|s| *s == "status").count(), 1);
}
