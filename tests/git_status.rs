//! Status parsing against real `git status` output for each XY state.

mod common;

use common::{find, xy, Repo, TempDir};
use diffkit::git::{
    models::{Change, FileKind, RepositorySnapshot, Similarity},
    status,
};
use std::path::{Path, PathBuf};

/// A repository with a few committed files to change.
fn fixture(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    let long: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, &format!("{f}\n{long}"));
    }
    repo.write("dir/nested.txt", "nested\n");
    repo.commit_all("base");
    (tmp, repo)
}

fn only(s: &RepositorySnapshot) -> &diffkit::git::models::FileStatus {
    assert_eq!(s.status.files.len(), 1, "{:#?}", s.status.files);
    &s.status.files[0]
}

#[test]
fn clean_repository() {
    let (_t, repo) = fixture("status-clean");
    let s = repo.snapshot();
    assert!(s.status.is_clean());
    assert_eq!(
        (
            s.status.staged_count(),
            s.status.unstaged_count(),
            s.status.untracked_count(),
            s.status.conflicted_count()
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn unstaged_modification() {
    let (_t, repo) = fixture("status-dot-m");
    repo.write("a.txt", "changed");
    let s = repo.snapshot();
    let f = only(&s);
    assert_eq!(xy(f), ".M");
    assert_eq!(
        (f.index, f.worktree),
        (Change::Unmodified, Change::Modified)
    );
    assert!(!f.is_staged() && f.is_unstaged());
    assert_eq!(f.file_kind, Some(FileKind::Regular));
}

#[test]
fn staged_modification() {
    let (_t, repo) = fixture("status-m-dot");
    repo.write("a.txt", "changed");
    repo.git(&["add", "a.txt"]);
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), "M.");
    assert!(f.is_staged() && !f.is_unstaged());
}

#[test]
fn partially_staged_modification() {
    let (_t, repo) = fixture("status-mm");
    repo.write("a.txt", "staged");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "staged and then changed again");
    let s = repo.snapshot();
    let f = only(&s);
    assert_eq!(xy(f), "MM");
    assert_eq!((f.index, f.worktree), (Change::Modified, Change::Modified));
    assert!(f.is_partially_staged());
    assert_eq!(
        (
            s.status.staged_count(),
            s.status.unstaged_count(),
            s.status.partially_staged_count()
        ),
        (1, 1, 1)
    );
}

#[test]
fn staged_modification_then_deleted() {
    let (_t, repo) = fixture("status-md");
    repo.write("a.txt", "staged");
    repo.git(&["add", "a.txt"]);
    repo.remove("a.txt");
    let s = repo.snapshot();
    let f = only(&s);
    assert_eq!(xy(f), "MD");
    assert!(f.is_partially_staged() && f.is_deleted());
}

#[test]
fn staged_addition_and_follow_up_changes() {
    let (_t, repo) = fixture("status-added");
    repo.write("new.txt", "new");
    repo.write("added-mod.txt", "v1");
    repo.write("added-del.txt", "v1");
    repo.git(&["add", "new.txt", "added-mod.txt", "added-del.txt"]);
    repo.write("added-mod.txt", "v2");
    repo.remove("added-del.txt");
    let s = repo.snapshot();
    assert_eq!(s.status.files.len(), 3);
    let added = find(&s, "new.txt");
    assert_eq!(xy(added), "A.");
    assert_eq!(added.index, Change::Added);
    assert!(added.is_staged() && !added.is_unstaged());
    let am = find(&s, "added-mod.txt");
    assert_eq!(xy(am), "AM");
    assert!(am.is_partially_staged());
    let ad = find(&s, "added-del.txt");
    assert_eq!(xy(ad), "AD");
    assert_eq!((ad.index, ad.worktree), (Change::Added, Change::Deleted));
}

#[test]
fn intent_to_add_is_an_unstaged_addition() {
    let (_t, repo) = fixture("status-ita");
    repo.write("later.txt", "content");
    repo.git(&["add", "-N", "later.txt"]);
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), ".A");
    assert_eq!((f.index, f.worktree), (Change::Unmodified, Change::Added));
    assert!(!f.is_staged() && f.is_unstaged() && !f.is_untracked());
}

#[test]
fn staged_deletion() {
    let (_t, repo) = fixture("status-d-dot");
    repo.git(&["rm", "-q", "a.txt"]);
    let s = repo.snapshot();
    let f = only(&s);
    assert_eq!(xy(f), "D.");
    assert!(f.is_staged() && !f.is_unstaged() && f.is_deleted());
    assert_eq!(
        f.file_kind,
        Some(FileKind::Regular),
        "kind comes from the HEAD mode"
    );
    assert_eq!(s.status.deleted_count(), 1);
}

#[test]
fn unstaged_deletion() {
    let (_t, repo) = fixture("status-dot-d");
    repo.remove("a.txt");
    let s = repo.snapshot();
    let f = only(&s);
    assert_eq!(xy(f), ".D");
    assert!(!f.is_staged() && f.is_unstaged() && f.is_deleted());
}

#[test]
fn staged_rename_keeps_original_path() {
    let (_t, repo) = fixture("status-rename");
    repo.git(&["mv", "a.txt", "renamed.txt"]);
    let s = repo.snapshot();
    let f = only(&s);
    assert_eq!(xy(f), "R.");
    assert_eq!(f.path, Path::new("renamed.txt"));
    assert_eq!(f.original_path.as_deref(), Some(Path::new("a.txt")));
    assert_eq!(f.similarity, Some(Similarity(100)));
    assert!(f.is_renamed() && f.is_staged());
    assert_eq!(s.status.renamed_count(), 1);
}

#[test]
fn rename_then_modified_or_deleted() {
    let (_t, repo) = fixture("status-rm-rd");
    repo.git(&["mv", "a.txt", "ra.txt"]);
    repo.git(&["mv", "b.txt", "rb.txt"]);
    repo.write("ra.txt", "completely different now");
    repo.remove("rb.txt");
    let s = repo.snapshot();
    let rm = find(&s, "ra.txt");
    assert_eq!(xy(rm), "RM");
    assert_eq!(rm.original_path.as_deref(), Some(Path::new("a.txt")));
    assert!(rm.is_partially_staged());
    let rd = find(&s, "rb.txt");
    assert_eq!(xy(rd), "RD");
    assert_eq!(rd.original_path.as_deref(), Some(Path::new("b.txt")));
}

#[test]
fn worktree_rename_of_intent_to_add_file() {
    let (_t, repo) = fixture("status-dot-r");
    std::fs::rename(repo.path.join("a.txt"), repo.path.join("moved.txt")).unwrap();
    repo.git(&["add", "-N", "moved.txt"]);
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), ".R");
    assert_eq!(f.original_path.as_deref(), Some(Path::new("a.txt")));
    assert!(!f.is_staged() && f.is_unstaged() && f.is_renamed());
}

#[test]
fn copy_record_when_git_reports_copies() {
    // Git only reports copies when copy detection is enabled
    // (status.renames=copies) and the source changed in the same set.
    let (_t, repo) = fixture("status-copy");
    repo.git(&["config", "status.renames", "copies"]);
    let original = std::fs::read_to_string(repo.path.join("a.txt")).unwrap();
    repo.write("copy-of-a.txt", &original);
    repo.write("a.txt", &format!("{original}appended\n"));
    repo.git(&["add", "a.txt", "copy-of-a.txt"]);
    let s = repo.snapshot();
    let c = find(&s, "copy-of-a.txt");
    assert_eq!(xy(c), "C.");
    assert_eq!(c.index, Change::Copied);
    assert_eq!(c.original_path.as_deref(), Some(Path::new("a.txt")));
    assert_eq!(c.similarity, Some(Similarity(100)));
    assert!(c.is_copied() && c.is_staged() && !c.is_renamed());
    assert_eq!(xy(find(&s, "a.txt")), "M.");
    assert_eq!(s.status.copied_count(), 1);
}

#[test]
fn copy_is_an_addition_without_copy_detection() {
    let (_t, repo) = fixture("status-copy-default");
    let original = std::fs::read_to_string(repo.path.join("a.txt")).unwrap();
    repo.write("copy-of-a.txt", &original);
    repo.write("a.txt", &format!("{original}appended\n"));
    repo.git(&["add", "a.txt", "copy-of-a.txt"]);
    let s = repo.snapshot();
    assert_eq!(xy(find(&s, "copy-of-a.txt")), "A.");
    assert_eq!(s.status.copied_count(), 0);
}

#[cfg(unix)]
#[test]
fn type_change_unstaged_and_staged() {
    let (_t, repo) = fixture("status-typechange");
    repo.remove("a.txt");
    std::os::unix::fs::symlink("b.txt", repo.path.join("a.txt")).unwrap();
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), ".T");
    assert_eq!(f.worktree, Change::TypeChanged);
    assert_eq!(f.file_kind, Some(FileKind::Symlink));
    repo.git(&["add", "a.txt"]);
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), "T.");
    assert_eq!(f.index, Change::TypeChanged);
    assert!(f.is_staged() && !f.is_unstaged());
}

#[cfg(unix)]
#[test]
fn executable_bit_change_is_a_modification() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, repo) = fixture("status-chmod");
    let p = repo.path.join("a.txt");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), ".M");
    assert_eq!(f.file_kind, Some(FileKind::Executable));
}

#[test]
fn multiple_modified_and_untracked_files() {
    let (_t, repo) = fixture("status-multiple");
    for f in ["a.txt", "b.txt", "c.txt"] {
        repo.write(f, "changed");
    }
    for f in ["u1", "u2", "u3", "u4"] {
        repo.write(f, "new");
    }
    let s = repo.snapshot();
    assert_eq!(s.status.unstaged_count(), 3);
    assert_eq!(s.status.untracked_count(), 4);
    assert_eq!(s.status.files.len(), 7);
}

#[test]
fn nested_untracked_files_are_listed_individually() {
    let (_t, repo) = fixture("status-nested-untracked");
    repo.write("new/a/b/c/deep.txt", "x");
    repo.write("new/a/sibling.txt", "x");
    repo.write("dir/also-new.txt", "x");
    let s = repo.snapshot();
    let mut paths: Vec<PathBuf> = s.status.files.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "dir/also-new.txt",
            "new/a/b/c/deep.txt",
            "new/a/sibling.txt"
        ]
        .map(PathBuf::from)
        .to_vec()
    );
    assert!(s.status.files.iter().all(|f| f.is_untracked()));
}

#[test]
fn ignored_files_are_not_untracked() {
    let (_t, repo) = fixture("status-ignored");
    repo.write(".gitignore", "*.log\nbuild/\n");
    repo.commit_all("ignore rules");
    repo.write("debug.log", "x");
    repo.write("build/out.o", "x");
    repo.write("build/deep/more.o", "x");
    repo.write("dir/trace.log", "x");
    repo.write("visible.txt", "x");
    let s = repo.snapshot();
    assert_eq!(s.status.files.len(), 1, "{:#?}", s.status.files);
    assert!(find(&s, "visible.txt").is_untracked());
    assert!(
        s.status.ignored.is_empty(),
        "ignored files are not requested"
    );
}

#[test]
fn tracked_file_matching_ignore_rule_is_still_reported() {
    let (_t, repo) = fixture("status-ignored-tracked");
    repo.write("kept.log", "v1");
    repo.commit_all("track a log");
    repo.write(".gitignore", "*.log\n");
    repo.commit_all("ignore logs");
    repo.write("kept.log", "v2");
    assert_eq!(xy(only(&repo.snapshot())), ".M");
}

#[test]
fn submodule_state_from_real_git() {
    let tmp = TempDir::new("status-submodule");
    let sub_src = Repo::init(tmp.join("sub-src"));
    sub_src.write("s.txt", "s");
    sub_src.commit_all("sub");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("r.txt", "r");
    repo.commit_all("root");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        sub_src.path.to_str().unwrap(),
        "sub",
    ]);
    repo.commit_all("add submodule");
    let sub = Repo {
        path: repo.path.join("sub"),
    };
    sub.git(&["config", "user.name", "Diffkit Test"]);
    sub.git(&["config", "user.email", "test@diffkit.invalid"]);
    sub.write("s.txt", "s2");
    sub.commit_all("new sub commit");
    sub.write("untracked-in-sub", "u");
    let f = only(&repo.snapshot()).clone();
    assert_eq!(xy(&f), ".M");
    assert_eq!(f.file_kind, Some(FileKind::Submodule));
    let state = f.submodule.expect("submodule state");
    assert!(state.commit_changed && !state.has_tracked_changes && state.has_untracked_changes);
}

#[test]
fn full_matrix_counts_in_one_repository() {
    let (_t, repo) = fixture("status-matrix");
    repo.write("a.txt", "unstaged"); // .M
    repo.write("b.txt", "staged"); // M.
    repo.git(&["add", "b.txt"]);
    repo.write("c.txt", "staged"); // MM
    repo.git(&["add", "c.txt"]);
    repo.write("c.txt", "and unstaged");
    repo.git(&["rm", "-q", "dir/nested.txt"]); // D.
    repo.write("added.txt", "new"); // A.
    repo.git(&["add", "added.txt"]);
    repo.write("untracked.txt", "u"); // ??
    let s = repo.snapshot();
    let codes: Vec<String> = s.status.files.iter().map(xy).collect();
    for code in [".M", "M.", "MM", "D.", "A.", "??"] {
        assert!(codes.contains(&code.to_string()), "{code} in {codes:?}");
    }
    let st = &s.status;
    assert_eq!(st.staged_count(), 4, "M., MM, D., A.");
    assert_eq!(st.unstaged_count(), 2, ".M, MM");
    assert_eq!(st.partially_staged_count(), 1);
    assert_eq!(st.untracked_count(), 1);
    assert_eq!(st.deleted_count(), 1);
    assert_eq!(st.conflicted_count(), 0);
}

#[test]
fn status_read_uses_one_porcelain_v2_call() {
    let (_t, repo) = fixture("status-read");
    repo.write("a.txt", "x");
    let report = status::read(&repo.path).unwrap();
    assert_eq!(report.headers.head.as_deref(), Some(&b"main"[..]));
    assert_eq!(report.status, repo.snapshot().status);
    assert_eq!(status::STATUS_ARGS[1], "--porcelain=v2");
    assert!(status::STATUS_ARGS.contains(&"-z"));
    assert!(status::STATUS_ARGS.contains(&"--untracked-files=all"));
}
