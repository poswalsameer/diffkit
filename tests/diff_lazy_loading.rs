//! Discovery is separate from loading, and loading never attaches new
//! content to stale metadata.

mod common;

use common::{discover, engine, engine_with, entry, rows, Repo, TempDir};
use diffkit::diff::{
    DiffComparison::*, DiffContent, DiffError, DiffOptions, FileId, LineStats, StaleReason,
};
use std::path::PathBuf;

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, &format!("{f} line 1\n{f} line 2\n"));
    }
    repo.commit_all("base");
    (tmp, repo)
}

fn stale_reason(r: Result<diffkit::diff::FileDiff, DiffError>) -> StaleReason {
    match r {
        Err(DiffError::Stale { reason, .. }) => reason,
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn discovery_carries_metadata_and_loading_is_per_file() {
    let (_t, repo) = repo("lazy-metadata");
    repo.write("a.txt", "a.txt line 1\nchanged\nadded\n");
    repo.git(&["add", "a.txt"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    let f = entry(&list, "a.txt");
    assert_eq!(
        f.stats,
        Some(LineStats {
            added: 2,
            removed: 1
        })
    );
    assert_eq!(f.has_hunks(), Some(true));
    assert_eq!(f.old_oid.as_ref().unwrap().as_str(), repo.rev("HEAD:a.txt"));
    assert_eq!(f.new_oid.as_ref().unwrap().as_str(), repo.rev(":a.txt"));
    let d = e.load_file_diff(&list, f.id).unwrap();
    assert_eq!(
        d.line_stats(),
        LineStats {
            added: 2,
            removed: 1
        },
        "loaded content agrees with discovery"
    );
}

#[test]
fn restaged_file_is_stale() {
    let (_t, repo) = repo("lazy-restaged");
    repo.write("a.txt", "first staging\n");
    repo.git(&["add", "a.txt"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    repo.write("a.txt", "second staging, different\n");
    repo.git(&["add", "a.txt"]);
    let id = entry(&list, "a.txt").id;
    assert_eq!(
        stale_reason(e.load_file_diff(&list, id)),
        StaleReason::ObjectChanged
    );
    let all = e.load_all(&list).unwrap();
    assert_eq!(
        stale_reason(all.files.into_iter().next().unwrap().result),
        StaleReason::ObjectChanged
    );
}

#[test]
fn unstaged_file_edited_or_deleted_after_discovery_is_stale() {
    let (_t, repo) = repo("lazy-edited");
    repo.write("a.txt", "edit one\n");
    repo.write("b.txt", "edit one\n");
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    repo.write("a.txt", "edit two, longer than before\n");
    repo.remove("b.txt");
    assert_eq!(
        stale_reason(e.load_file_diff(&list, entry(&list, "a.txt").id)),
        StaleReason::WorkingTreeChanged
    );
    assert_eq!(
        stale_reason(e.load_file_diff(&list, entry(&list, "b.txt").id)),
        StaleReason::WorkingTreeChanged
    );
}

#[test]
fn change_reverted_after_discovery_is_missing() {
    let (_t, repo) = repo("lazy-reverted");
    repo.write("c.txt", "staged\n");
    repo.git(&["add", "c.txt"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    repo.git(&["reset", "-q", "--", "c.txt"]);
    assert_eq!(
        stale_reason(e.load_file_diff(&list, entry(&list, "c.txt").id)),
        StaleReason::MissingFromDiff
    );
}

#[test]
fn load_all_matches_per_file_loads_and_reports_new_changes() {
    let (_t, repo) = repo("lazy-load-all");
    repo.write("a.txt", "a.txt line 1\nA\n");
    repo.write("b.txt", "B\nb.txt line 2\n");
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    let singles: Vec<_> = list
        .files
        .iter()
        .map(|f| e.load_file_diff(&list, f.id).unwrap())
        .collect();
    repo.write("c.txt", "changed after discovery\n");
    let all = e.load_all(&list).unwrap();
    assert_eq!(all.unexpected_paths, vec![PathBuf::from("c.txt")]);
    let loaded: Vec<_> = all.files.into_iter().map(|l| l.result.unwrap()).collect();
    assert_eq!(loaded, singles);
}

#[test]
fn unknown_file_id() {
    let (_t, repo) = repo("lazy-unknown-id");
    let e = engine(&repo.path);
    let list = discover(&e, Unstaged);
    assert!(matches!(
        e.load_file_diff(&list, FileId(7)),
        Err(DiffError::UnknownFile(FileId(7)))
    ));
}

#[test]
fn patch_size_limits() {
    let (_t, repo) = repo("lazy-limits");
    repo.write("a.txt", &"many lines\n".repeat(500));
    repo.write("b.txt", "b small\n");
    let tiny = engine_with(
        &repo.path,
        DiffOptions {
            max_file_patch_bytes: 200,
            ..DiffOptions::default()
        },
    );
    let list = discover(&tiny, Unstaged);
    let big = tiny
        .load_file_diff(&list, entry(&list, "a.txt").id)
        .unwrap();
    assert_eq!(
        big.content,
        DiffContent::TooLarge {
            limit: 200,
            size: None
        }
    );
    assert!(big.hunks().is_empty(), "nothing partial");
    let small = tiny
        .load_file_diff(&list, entry(&list, "b.txt").id)
        .unwrap();
    assert!(matches!(small.content, DiffContent::Text { .. }));

    // When the whole comparison is too large, load_all falls back to
    // per-file loads instead of failing or truncating.
    let fallback = engine_with(
        &repo.path,
        DiffOptions {
            max_total_patch_bytes: 300,
            ..DiffOptions::default()
        },
    );
    let all = fallback.load_all(&discover(&fallback, Unstaged)).unwrap();
    let a = all
        .files
        .iter()
        .find(|l| l.result.as_ref().unwrap().file.path().ends_with("a.txt"))
        .unwrap();
    // Nothing lost: both old lines and all 500 new lines are present.
    let r = rows(a.result.as_ref().unwrap());
    assert_eq!(r.iter().filter(|r| r.1.is_some()).count(), 2);
    assert_eq!(r.iter().filter(|r| r.2.is_some()).count(), 500);
}
