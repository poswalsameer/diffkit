//! `diffkit diff-stats [--load]` output.

mod common;

use common::{diffkit, stderr, stdout, Repo, TempDir};

fn sample() -> (TempDir, Repo) {
    let tmp = TempDir::new("diff-cli");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "one\ntwo\nthree\n");
    repo.write(
        "old name.txt",
        &(1..=20).map(|i| format!("line {i}\n")).collect::<String>(),
    );
    std::fs::write(repo.path.join("b.bin"), b"\x00old").unwrap();
    repo.commit_all("base");
    repo.write("a.txt", "one\ntwo changed\nthree\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "one\ntwo changed again\nthree\nfour\n");
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    std::fs::write(repo.path.join("b.bin"), b"\x00new!").unwrap();
    repo.write("notes.md", "n\n");
    (tmp, repo)
}

#[test]
fn diff_stats_lists_every_comparison() {
    let (_t, repo) = sample();
    let out = diffkit(&repo.path, &["diff-stats"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let head = repo.head();
    assert!(
        text.starts_with(&format!("DIFFKIT - DIFF SUMMARY\nBase: HEAD {head}\n")),
        "{text}"
    );
    assert!(
        text.contains("\nSTAGED (HEAD -> index): 2 files, +1 -1, 0 binary\n"),
        "{text}"
    );
    assert!(text.contains("  M   a.txt  +1 -1\n"), "{text}");
    assert!(
        text.contains("  R   old name.txt -> new name.txt  100% similar, +0 -0\n"),
        "{text}"
    );
    assert!(
        text.contains("\nUNSTAGED (index -> working tree): 2 files, +2 -1, 1 binary\n"),
        "{text}"
    );
    assert!(text.contains("  M   b.bin  binary\n"), "{text}");
    assert!(
        text.contains("\nCOMBINED (HEAD -> working tree): 3 files, +2 -1, 1 binary\n"),
        "{text}"
    );
    assert!(
        text.contains("\nUNTRACKED (untracked -> working tree): 1 file\n  ??  notes.md\n"),
        "{text}"
    );
}

#[test]
fn diff_stats_load_reports_rows() {
    let (_t, repo) = sample();
    let out = diffkit(&repo.path, &["diff-stats", "--load"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(
            "  M   a.txt  +1 -1  | 1 hunk; rows: 2 context, 1 added, 1 removed, 0 modified\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("  M   b.bin  binary  | binary; size 4 -> 5\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "  ??  notes.md  | 1 hunk; rows: 0 context, 1 added, 0 removed, 0 modified\n"
        ),
        "{text}"
    );
}

#[test]
fn diff_stats_errors_and_usage() {
    let tmp = TempDir::new("diff-cli-errors");
    let plain = tmp.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let out = diffkit(&plain, &["diff-stats"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("is not inside a Git repository"));
    let out = diffkit(&plain, &["diff-stats", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&diffkit(&plain, &["--help"])).contains("diff-stats"));
}
