use crate::support;

use diffkit::types::repository::HeadState;
use support::{Repo, TempDir};

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

#[test]
fn normal_branch_and_correct_head_hash() {
    let (_t, repo) = repo("branch-normal");
    repo.write("f", "1");
    let first = repo.commit_all("first");
    let s = repo.snapshot();
    assert_eq!(s.branch.head, HeadState::Branch("main".into()));
    assert_eq!(s.branch.head_commit.as_ref().unwrap().as_str(), first);
    assert!(!s.branch.detached() && !s.branch.unborn());
    assert_eq!(s.branch.empty_repository, None);

    repo.write("f", "2");
    let second = repo.commit_all("second");
    assert_ne!(first, second);
    assert_eq!(repo.snapshot().branch.head_commit.unwrap().as_str(), second);
}

#[test]
fn detached_head_is_explicit() {
    let (_t, repo) = repo("branch-detached");
    let first = repo.empty_commit("first");
    repo.empty_commit("second");
    repo.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    let s = repo.snapshot();
    assert_eq!(s.branch.head, HeadState::Detached);
    assert!(s.branch.detached());
    assert_eq!(s.branch.name(), None, "not an ordinary branch named HEAD");
    assert_eq!(s.branch.head_commit.unwrap().as_str(), first);
}

#[test]
fn empty_repository_has_unborn_branch() {
    let (_t, repo) = repo("branch-empty");
    let s = repo.snapshot();
    assert_eq!(s.branch.head, HeadState::Unborn("main".into()));
    assert_eq!(s.branch.head_commit, None);
    assert_eq!(s.branch.empty_repository, Some(true));
}

#[test]
fn unborn_branch_with_custom_initial_name() {
    let tmp = TempDir::new("branch-unborn-custom");
    let repo = Repo::init_with_branch(tmp.join("repo"), "trunk/dev");
    assert_eq!(
        repo.snapshot().branch.head,
        HeadState::Unborn("trunk/dev".into())
    );
}

#[test]
fn orphan_branch_is_unborn_but_repository_is_not_empty() {
    let (_t, repo) = repo("branch-orphan");
    repo.empty_commit("on main");
    repo.git(&["switch", "-q", "--orphan", "fresh"]);
    let s = repo.snapshot();
    assert_eq!(s.branch.head, HeadState::Unborn("fresh".into()));
    assert_eq!(s.branch.empty_repository, Some(false));
}

#[test]
fn branch_names_with_slashes_and_unusual_valid_characters() {
    let (_t, repo) = repo("branch-names");
    repo.empty_commit("base");
    for name in [
        "feature/login",
        "feature/deeply/nested/name",
        "release-1.0_rc+build@x",
        "fix/\u{fc}mlaut-\u{65e5}\u{672c}",
        "UPPER.case-#42",
    ] {
        repo.git(&["check-ref-format", "--branch", name]);
        repo.git(&["switch", "-q", "-c", name]);
        let s = repo.snapshot();
        assert_eq!(s.branch.head, HeadState::Branch(name.into()));
        repo.git(&["switch", "-q", "main"]);
    }
}

#[test]
fn external_branch_switches_are_reflected() {
    let (_t, repo) = repo("branch-switch");
    let main_head = repo.empty_commit("main");
    assert_eq!(repo.snapshot().branch.name(), Some("main"));

    repo.git(&["switch", "-q", "-c", "topic"]);
    let topic_head = repo.empty_commit("topic");
    let s = repo.snapshot();
    assert_eq!(s.branch.name(), Some("topic"));
    assert_eq!(s.branch.head_commit.unwrap().as_str(), topic_head);

    repo.git(&["checkout", "-q", "main"]);
    let s = repo.snapshot();
    assert_eq!(s.branch.name(), Some("main"));
    assert_eq!(s.branch.head_commit.unwrap().as_str(), main_head);
}
