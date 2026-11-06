mod common;

use common::{find, inspect, xy, Repo, TempDir};
use diffkit::git::{errors::GitError, models::RepositoryKind, repository};

#[test]
fn discovers_standard_repository_from_root() {
    let tmp = TempDir::new("discover-root");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "a");
    repo.commit_all("init");
    let info = repository::discover_from(&repo.path).unwrap();
    assert_eq!(info.root, repo.path);
    assert_eq!(info.current_directory, repo.path);
    assert_eq!(info.git_dir, repo.path.join(".git"));
    assert_eq!(info.common_git_dir, repo.path.join(".git"));
    assert!(
        info.git_dir.is_dir(),
        ".git is a directory in a standard repository"
    );
    assert_eq!(info.kind, RepositoryKind::Standard);
    assert!(info.is_inside_work_tree);
    assert!(!info.shallow);
}

#[test]
fn discovers_root_from_multiple_nested_levels() {
    let tmp = TempDir::new("discover-nested");
    let repo = Repo::init(tmp.join("repo"));
    for nested in ["src", "src/components", "src/components/deep/er/still"] {
        std::fs::create_dir_all(repo.path.join(nested)).unwrap();
        let info = repository::discover_from(&repo.path.join(nested)).unwrap();
        assert_eq!(info.root, repo.path, "from {nested}");
        assert_eq!(info.current_directory, repo.path.join(nested));
        assert_eq!(info.git_dir, repo.path.join(".git"));
    }
}

#[test]
fn discovers_empty_repository() {
    let tmp = TempDir::new("discover-empty");
    let repo = Repo::init(tmp.join("repo"));
    let s = repo.snapshot();
    assert_eq!(s.repository.root, repo.path);
    assert!(s.branch.unborn());
    assert!(s.status.is_clean());
}

#[test]
fn linked_worktree_has_own_git_dir_and_shared_common_dir() {
    let tmp = TempDir::new("discover-worktree");
    let main = Repo::init(tmp.join("main"));
    main.write("a.txt", "a");
    main.commit_all("init");
    let wt = tmp.join("linked");
    main.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "linked",
        wt.to_str().unwrap(),
    ]);
    assert!(
        wt.join(".git").is_file(),
        ".git is a file in a linked worktree"
    );
    std::fs::create_dir_all(wt.join("nested/dir")).unwrap();
    for start in [wt.clone(), wt.join("nested/dir")] {
        let info = repository::discover_from(&start).unwrap();
        assert_eq!(info.root, wt);
        assert_eq!(info.kind, RepositoryKind::LinkedWorktree);
        assert_eq!(info.common_git_dir, main.path.join(".git"));
        assert_eq!(info.git_dir, main.path.join(".git/worktrees/linked"));
        assert_ne!(info.git_dir, info.common_git_dir);
    }
    let main_info = repository::discover_from(&main.path).unwrap();
    assert_eq!(
        main_info.kind,
        RepositoryKind::Standard,
        "main worktree is unaffected"
    );
}

#[test]
fn multiple_worktrees_are_inspected_independently() {
    let tmp = TempDir::new("discover-multi-worktree");
    let main = Repo::init(tmp.join("main"));
    main.write("a.txt", "a\n");
    let base = main.commit_all("init");
    let (wt1, wt2) = (tmp.join("wt one"), tmp.join("wt-two"));
    main.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature/one",
        wt1.to_str().unwrap(),
    ]);
    main.git(&["worktree", "add", "-q", "--detach", wt2.to_str().unwrap()]);
    std::fs::write(wt1.join("a.txt"), "changed in one\n").unwrap();
    std::fs::write(wt2.join("new.txt"), "untracked in two\n").unwrap();

    let s0 = main.snapshot();
    let s1 = inspect(&wt1);
    let s2 = inspect(&wt2);

    assert_eq!(s0.repository.kind, RepositoryKind::Standard);
    assert_eq!(s1.repository.kind, RepositoryKind::LinkedWorktree);
    assert_eq!(s2.repository.kind, RepositoryKind::LinkedWorktree);
    let common = main.path.join(".git");
    for s in [&s0, &s1, &s2] {
        assert_eq!(s.repository.common_git_dir, common);
    }
    assert_ne!(s1.repository.git_dir, s2.repository.git_dir);
    assert_eq!(s1.repository.root, wt1);
    assert_eq!(s2.repository.root, wt2);

    assert_eq!(s0.branch.name(), Some("main"));
    assert_eq!(s1.branch.name(), Some("feature/one"));
    assert!(s2.branch.detached());
    assert_eq!(s2.branch.head_commit.as_ref().unwrap().as_str(), base);

    assert!(
        s0.status.is_clean(),
        "changes in linked worktrees do not leak into main"
    );
    assert_eq!(s1.status.files.len(), 1);
    assert_eq!(xy(find(&s1, "a.txt")), ".M");
    assert_eq!(s2.status.files.len(), 1);
    assert!(find(&s2, "new.txt").is_untracked());
}

#[test]
fn repository_path_with_spaces_and_unicode() {
    let tmp = TempDir::new("discover space ü 日本");
    let repo = Repo::init(tmp.join("my repo — café 🎉"));
    repo.write("dir with space/file.txt", "x");
    repo.commit_all("init");
    repo.write("dir with space/file.txt", "y");
    std::fs::create_dir_all(repo.path.join("dir with space/nested ü")).unwrap();
    let s = inspect(&repo.path.join("dir with space/nested ü"));
    assert_eq!(s.repository.root, repo.path);
    assert_eq!(s.repository.git_dir, repo.path.join(".git"));
    assert_eq!(xy(find(&s, "dir with space/file.txt")), ".M");
}

#[test]
fn shallow_clone_is_detected() {
    let tmp = TempDir::new("discover-shallow");
    let src = Repo::init(tmp.join("src"));
    for i in 0..3 {
        src.write("f", &i.to_string());
        src.commit_all(&format!("c{i}"));
    }
    let url = format!("file://{}", src.path.display());
    common::git(
        tmp.path(),
        &["clone", "-q", "--depth", "1", &url, "shallow"],
    );
    let info = repository::discover_from(&tmp.join("shallow")).unwrap();
    assert!(info.shallow);
    assert!(!repository::discover_from(&src.path).unwrap().shallow);
}

#[test]
fn non_git_directory_is_reported() {
    let tmp = TempDir::new("discover-plain");
    let plain = tmp.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    match repository::discover_from(&plain) {
        Err(GitError::NotARepository { path, .. }) => assert_eq!(path, plain),
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[test]
fn git_directory_and_bare_repository_are_not_work_trees() {
    let tmp = TempDir::new("discover-not-worktree");
    let repo = Repo::init(tmp.join("repo"));
    let bare = Repo::init_bare(tmp.join("bare.git"));
    for dir in [repo.path.join(".git"), bare.path.clone()] {
        match repository::discover_from(&dir) {
            Err(GitError::NotAWorkTree { path }) => assert_eq!(path, dir),
            other => panic!("expected NotAWorkTree for {}, got {other:?}", dir.display()),
        }
    }
}

#[test]
fn missing_directory_is_inaccessible() {
    let tmp = TempDir::new("discover-missing");
    let missing = tmp.join("does/not/exist");
    assert!(matches!(
        repository::discover_from(&missing),
        Err(GitError::PathInaccessible { .. })
    ));
}
