//! Staged, unstaged, combined, and untracked comparisons against real Git.

use crate::support;

use diffkit::{
    git::commands::thread_invocations,
    types::diff::{
        DiffComparison::{self, *},
        DiffContent, DiffOptions,
        DiffRowKind::*,
        FileChangeKind, LineStats,
    },
};
use support::{discover, engine, engine_with, entry, load, numbered, row, rows, Repo, TempDir};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

#[test]
fn clean_repository_has_no_changes() {
    let (_t, repo) = repo("diff-clean");
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    let e = engine(&repo.path);
    for c in DiffComparison::ALL {
        assert!(discover(&e, c).is_empty(), "{c:?}");
    }
}

#[test]
fn partial_staging_keeps_three_comparisons_distinct() {
    let (_t, repo) = repo("diff-partial");
    repo.write("f.txt", "line1\nvalue = A\nline3\n");
    let head = repo.commit_all("A");
    repo.write("f.txt", "line1\nvalue = B\nline3\n");
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", "line1\nvalue = C\nline3\n");
    let e = engine(&repo.path);

    let staged = load(&e, Staged, "f.txt");
    assert_eq!(
        rows(&staged),
        vec![
            row(Context, Some(1), Some(1), "line1", "line1"),
            row(Modified, Some(2), Some(2), "value = A", "value = B"),
            row(Context, Some(3), Some(3), "line3", "line3"),
        ],
        "HEAD -> index"
    );
    assert_eq!(
        staged.file.old_oid.as_ref().unwrap().as_str(),
        repo.rev("HEAD:f.txt")
    );
    assert_eq!(
        staged.file.new_oid.as_ref().unwrap().as_str(),
        repo.rev(":f.txt")
    );

    let unstaged = load(&e, Unstaged, "f.txt");
    assert_eq!(
        rows(&unstaged)[1],
        row(Modified, Some(2), Some(2), "value = B", "value = C"),
        "index -> working tree"
    );
    assert_eq!(
        unstaged.file.old_oid.as_ref().unwrap().as_str(),
        repo.rev(":f.txt")
    );
    assert_eq!(
        unstaged.file.new_oid, None,
        "the working tree is not hashed at discovery"
    );

    let combined = load(&e, Combined, "f.txt");
    assert_eq!(
        rows(&combined)[1],
        row(Modified, Some(2), Some(2), "value = A", "value = C"),
        "HEAD -> working tree"
    );
    assert_eq!(e.discover(Combined).unwrap().base.unwrap().as_str(), head);
}

#[test]
fn combined_is_not_the_sum_of_staged_and_unstaged() {
    let (_t, repo) = repo("diff-not-sum");
    repo.write("f.txt", "a\nb\n");
    repo.commit_all("base");
    repo.write("f.txt", "a\nstaged insert\nb\n");
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", "a\nb\n");
    let e = engine(&repo.path);
    let stats = |c| discover(&e, c).line_stats();
    assert_eq!(
        stats(Staged),
        LineStats {
            added: 1,
            removed: 0
        }
    );
    assert_eq!(
        stats(Unstaged),
        LineStats {
            added: 0,
            removed: 1
        }
    );
    assert!(
        discover(&e, Combined).is_empty(),
        "the working tree equals HEAD"
    );
}

#[test]
fn line_numbers_follow_each_side() {
    let (_t, repo) = repo("diff-line-numbers");
    repo.write("f.txt", &numbered("line ", 10));
    repo.commit_all("base");
    let staged = format!("new top 1\nnew top 2\n{}", numbered("line ", 10));
    repo.write("f.txt", &staged);
    repo.git(&["add", "f.txt"]);
    repo.write("f.txt", &staged.replace("line 6\n", "line six\n"));
    let e = engine(&repo.path);

    let u = load(&e, Unstaged, "f.txt");
    assert!(rows(&u).contains(&row(Modified, Some(8), Some(8), "line 6", "line six")));
    let c = load(&e, Combined, "f.txt");
    let r = rows(&c);
    assert!(r.contains(&row(Added, None, Some(1), "", "new top 1")));
    assert!(r.contains(&row(Modified, Some(6), Some(8), "line 6", "line six")));
    assert!(r.contains(&row(Context, Some(5), Some(7), "line 5", "line 5")));
}

#[test]
fn staged_addition_and_deletion() {
    let (_t, repo) = repo("diff-add-delete");
    repo.write("old.txt", "x\ny\n");
    repo.commit_all("base");
    repo.write("new.txt", "first\nsecond\n");
    repo.git(&["add", "new.txt"]);
    repo.git(&["rm", "-q", "old.txt"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);

    let added = entry(&list, "new.txt");
    assert_eq!(added.kind, FileChangeKind::Added);
    assert_eq!(
        (added.old_path.as_deref(), added.new_mode.map(|m| m.0)),
        (None, Some(0o100644))
    );
    let d = e.load_file_diff(&list, added.id).unwrap();
    assert_eq!(
        rows(&d),
        vec![
            row(Added, None, Some(1), "", "first"),
            row(Added, None, Some(2), "", "second")
        ]
    );
    assert_eq!((d.hunks()[0].old_start, d.hunks()[0].old_count), (0, 0));

    let deleted = entry(&list, "old.txt");
    assert_eq!(deleted.kind, FileChangeKind::Deleted);
    assert_eq!(deleted.new_path, None);
    let d = e.load_file_diff(&list, deleted.id).unwrap();
    assert_eq!(
        rows(&d),
        vec![
            row(Removed, Some(1), None, "x", ""),
            row(Removed, Some(2), None, "y", "")
        ]
    );
}

#[test]
fn unstaged_deletion() {
    let (_t, repo) = repo("diff-unstaged-delete");
    repo.write("gone.txt", "1\n2\n");
    repo.commit_all("base");
    repo.remove("gone.txt");
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "gone.txt");
    assert_eq!(d.file.kind, FileChangeKind::Deleted);
    assert_eq!(
        d.line_stats(),
        LineStats {
            added: 0,
            removed: 2
        }
    );
    assert!(discover(&e, Staged).is_empty());
}

#[test]
fn rename_with_and_without_modification() {
    let (_t, repo) = repo("diff-rename");
    let body = numbered("shared line ", 20);
    repo.write("pure.txt", &body);
    repo.write("edited.txt", &numbered("other line ", 20));
    repo.commit_all("base");
    repo.git(&["mv", "pure.txt", "pure renamed.txt"]);
    std::fs::create_dir_all(repo.path.join("dir")).unwrap();
    repo.git(&["mv", "edited.txt", "dir/edited.txt"]);
    repo.write(
        "dir/edited.txt",
        &numbered("other line ", 20).replace("other line 10\n", "changed 10\n"),
    );
    repo.git(&["add", "-A"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);

    let pure = entry(&list, "pure renamed.txt");
    assert_eq!(pure.kind, FileChangeKind::Renamed);
    assert_eq!(
        pure.old_path.as_deref(),
        Some(std::path::Path::new("pure.txt"))
    );
    assert_eq!(pure.similarity, Some(100));
    assert_eq!(pure.has_hunks(), Some(false));
    let d = e.load_file_diff(&list, pure.id).unwrap();
    assert!(d.hunks().is_empty());

    let edited = entry(&list, "dir/edited.txt");
    assert_eq!(edited.kind, FileChangeKind::Renamed);
    assert_eq!(
        edited.old_path.as_deref(),
        Some(std::path::Path::new("edited.txt"))
    );
    assert!(edited.similarity.unwrap() < 100);
    let d = e.load_file_diff(&list, edited.id).unwrap();
    assert!(rows(&d).contains(&row(Removed, Some(10), None, "other line 10", "")));
    assert!(rows(&d).contains(&row(Added, None, Some(10), "", "changed 10")));
}

#[test]
fn copy_is_reported_when_git_detects_it() {
    let (_t, repo) = repo("diff-copy");
    let body = numbered("copied content ", 30);
    repo.write("src.txt", &body);
    repo.commit_all("base");
    repo.write("copy.txt", &body);
    repo.write("src.txt", &format!("{body}extra\n"));
    repo.git(&["add", "-A"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    let c = entry(&list, "copy.txt");
    assert_eq!(c.kind, FileChangeKind::Copied);
    assert_eq!(c.old_path.as_deref(), Some(std::path::Path::new("src.txt")));
    assert_eq!(c.similarity, Some(100));
    assert!(e.load_file_diff(&list, c.id).unwrap().hunks().is_empty());
    assert_eq!(entry(&list, "src.txt").kind, FileChangeKind::Modified);
}

#[cfg(unix)]
#[test]
fn mode_changes() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, repo) = repo("diff-mode");
    repo.write("only-mode.sh", "echo hi\n");
    repo.write("mode-and-content.sh", "echo a\n");
    repo.commit_all("base");
    let exec = |f: &str| {
        std::fs::set_permissions(repo.path.join(f), std::fs::Permissions::from_mode(0o755)).unwrap()
    };
    exec("only-mode.sh");
    exec("mode-and-content.sh");
    repo.write("mode-and-content.sh", "echo b\n");
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    let m = entry(&list, "only-mode.sh");
    assert_eq!(m.kind, FileChangeKind::ModeChanged);
    assert_eq!(
        (m.old_mode.unwrap().0, m.new_mode.unwrap().0),
        (0o100644, 0o100755)
    );
    assert!(e.load_file_diff(&list, m.id).unwrap().hunks().is_empty());
    let both = entry(&list, "mode-and-content.sh");
    assert_eq!(both.kind, FileChangeKind::Modified);
    assert_eq!(both.new_mode.unwrap().0, 0o100755);
    let d = e.load_file_diff(&list, both.id).unwrap();
    assert_eq!(
        rows(&d),
        vec![row(Modified, Some(1), Some(1), "echo a", "echo b")]
    );
    assert_eq!(
        (d.old_oid.is_some(), d.new_oid.is_some()),
        (true, true),
        "from the index line"
    );
}

#[test]
fn separated_hunks_and_context_size() {
    let (_t, repo) = repo("diff-hunks");
    repo.write("f.txt", &numbered("l", 40));
    repo.commit_all("base");
    repo.write(
        "f.txt",
        &numbered("l", 40)
            .replace("l5\n", "five\n")
            .replace("l30\n", "thirty\n"),
    );
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "f.txt");
    let h = d.hunks();
    assert_eq!(h.len(), 2);
    assert_eq!(
        (
            h[0].old_start,
            h[0].old_count,
            h[0].new_start,
            h[0].new_count
        ),
        (2, 7, 2, 7)
    );
    assert_eq!((h[1].old_start, h[1].old_count), (27, 7));
    // Git appends a function-context heading; it is kept separately too.
    assert_eq!(h[0].header, b"@@ -2,7 +2,7 @@ l1");
    assert_eq!(h[0].section_heading, b"l1");
    let ctx =
        |h: &diffkit::types::diff::DiffHunk| h.rows.iter().filter(|r| r.kind == Context).count();
    assert_eq!(ctx(&h[0]), 6);

    let tight = engine_with(
        &repo.path,
        DiffOptions {
            context_lines: 0,
            ..DiffOptions::default()
        },
    );
    let d = load(&tight, Unstaged, "f.txt");
    assert!(d.hunks().iter().all(|h| ctx(h) == 0));
    assert!(d.hunks()[0].header.starts_with(b"@@ -5 +5 @@"));

    let wide = engine_with(
        &repo.path,
        DiffOptions {
            context_lines: 20,
            ..DiffOptions::default()
        },
    );
    assert_eq!(
        load(&wide, Unstaged, "f.txt").hunks().len(),
        1,
        "nearby with wide context: one hunk"
    );
}

#[test]
fn multiple_files_in_one_comparison() {
    let (_t, repo) = repo("diff-multi");
    for i in 0..5 {
        repo.write(format!("f{i}.txt"), "a\n");
    }
    repo.commit_all("base");
    for i in 0..5 {
        repo.write(format!("f{i}.txt"), &format!("a\nadded {i}\n"));
    }
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    assert_eq!(list.len(), 5);
    assert_eq!(
        list.line_stats(),
        LineStats {
            added: 5,
            removed: 0
        }
    );
    for f in &list.files {
        let d = e.load_file_diff(&list, f.id).unwrap();
        let i = f
            .path()
            .to_str()
            .unwrap()
            .trim_start_matches('f')
            .trim_end_matches(".txt")
            .to_owned();
        assert_eq!(
            rows(&d)[1],
            row(Added, None, Some(2), "", &format!("added {i}"))
        );
    }
}

#[test]
fn unborn_head_compares_against_the_empty_tree() {
    let (_t, repo) = repo("diff-unborn");
    repo.write("a.txt", "staged\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "staged\nthen edited\n");
    let e = engine(&repo.path);
    assert_eq!(e.head(), None);
    let staged = load(&e, Staged, "a.txt");
    assert_eq!(staged.file.kind, FileChangeKind::Added);
    assert_eq!(rows(&staged), vec![row(Added, None, Some(1), "", "staged")]);
    let combined = load(&e, Combined, "a.txt");
    assert_eq!(combined.file.kind, FileChangeKind::Added);
    assert_eq!(
        combined.line_stats(),
        LineStats {
            added: 2,
            removed: 0
        }
    );
    let unstaged = load(&e, Unstaged, "a.txt");
    assert_eq!(
        rows(&unstaged)[1],
        row(Added, None, Some(2), "", "then edited")
    );
}

#[test]
fn large_file_with_scattered_changes() {
    let (_t, repo) = repo("diff-large");
    let body = numbered("row ", 50_000);
    repo.write("big.txt", &body);
    repo.commit_all("base");
    let mut changed = String::new();
    for (i, line) in body.lines().enumerate() {
        if i % 1000 == 500 {
            changed.push_str(&format!("{line} edited\n"));
        } else {
            changed.push_str(line);
            changed.push('\n');
        }
    }
    repo.write("big.txt", &changed);
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "big.txt");
    assert_eq!(d.hunks().len(), 50);
    assert_eq!(
        d.line_stats(),
        LineStats {
            added: 50,
            removed: 50
        }
    );
    assert!(rows(&d).contains(&row(
        Modified,
        Some(49_501),
        Some(49_501),
        "row 49501",
        "row 49501 edited"
    )));
}

#[test]
fn subprocess_counts() {
    let (_t, repo) = repo("diff-subprocesses");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit_all("base");
    repo.write("a.txt", "A\n");
    repo.write("b.txt", "B\n");
    repo.write("u.txt", "u\n");
    let e = engine(&repo.path);
    let n = thread_invocations();
    let list = e.discover(Unstaged).unwrap();
    assert_eq!(
        thread_invocations() - n,
        1,
        "discovery: one git diff --raw --numstat"
    );
    let n = thread_invocations();
    e.load_file_diff(&list, list.files[0].id).unwrap();
    assert_eq!(thread_invocations() - n, 1, "one file: one git diff");
    let n = thread_invocations();
    let all = e.load_all(&list).unwrap();
    assert_eq!(thread_invocations() - n, 1, "all files: one git diff");
    assert!(all.files.iter().all(|f| f.result.is_ok()));
    let n = thread_invocations();
    let untracked = e.discover(Untracked).unwrap();
    assert_eq!(thread_invocations() - n, 1, "ls-files");
    let n = thread_invocations();
    let d = e.load_file_diff(&untracked, untracked.files[0].id).unwrap();
    assert_eq!(
        thread_invocations() - n,
        0,
        "untracked files are read directly"
    );
    assert!(matches!(d.content, DiffContent::Text { .. }));
}

#[test]
fn stat_only_changes_are_not_reported_as_modifications() {
    let (_t, repo) = repo("diff-stat-only");
    repo.write("touched.txt", "same content\n");
    repo.write("edited.txt", "old\n");
    repo.commit_all("base");
    let f = std::fs::File::options()
        .write(true)
        .open(repo.path.join("touched.txt"))
        .unwrap();
    f.set_modified(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000),
    )
    .unwrap();
    repo.write("edited.txt", "new\n");
    let index = std::fs::read(repo.path.join(".git/index")).unwrap();
    let e = engine(&repo.path);
    for c in [Unstaged, Combined] {
        let list = discover(&e, c);
        assert_eq!(list.len(), 1, "{c:?}: {:?}", list.files);
        assert_eq!(list.files[0].path(), std::path::Path::new("edited.txt"));
        assert_eq!(list.stat_only_entries, 1, "{c:?}");
        e.load_all(&list).unwrap();
    }
    assert_eq!(
        std::fs::read(repo.path.join(".git/index")).unwrap(),
        index,
        "index not refreshed"
    );
}
