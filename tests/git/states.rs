//! Conflicts, in-progress operations, and whole-snapshot coherence.

use crate::support;

use diffkit::{
    git::status,
    types::repository::{
        Change, CommitTrackingInfo, ConflictKind, HeadState, RepositoryKind, TrackingUnavailable,
        UpstreamInfo,
    },
};
use support::{find, xy, Remote, Repo, TempDir};

fn failing_merge(repo: &Repo, branch: &str) {
    let out = repo.git_output(&["merge", "--no-edit", branch]);
    assert!(!out.status.success(), "merge was expected to conflict");
}

#[test]
fn single_merge_conflict() {
    let tmp = TempDir::new("state-conflict");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("f.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("f.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("f.txt", "main\n");
    repo.commit_all("main");
    failing_merge(&repo, "side");

    let s = repo.snapshot();
    let f = find(&s, "f.txt");
    assert_eq!(xy(f), "UU");
    assert_eq!(f.conflict, Some(ConflictKind::BothModified));
    assert_eq!((f.index, f.worktree), (Change::Unmerged, Change::Unmerged));
    assert_eq!(s.status.conflicted_count(), 1);
    assert_eq!(
        s.status.staged_count(),
        0,
        "conflicts are not counted as staged"
    );
    assert_eq!(
        s.branch.name(),
        Some("main"),
        "a merge keeps HEAD on the branch"
    );

    repo.write("f.txt", "resolved\n");
    repo.git(&["add", "f.txt"]);
    let s = repo.snapshot();
    assert_eq!(xy(find(&s, "f.txt")), "M.");
    assert_eq!(s.status.conflicted_count(), 0);
}

#[test]
fn multiple_simultaneous_conflicts_of_different_kinds() {
    let tmp = TempDir::new("state-multi-conflict");
    let repo = Repo::init(tmp.join("repo"));
    for f in ["uu", "ud", "du", "clean"] {
        repo.write(f, "base\n");
    }
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("uu", "side\n");
    repo.remove("ud");
    repo.write("du", "side\n");
    repo.write("aa", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("uu", "main\n");
    repo.write("ud", "main\n");
    repo.remove("du");
    repo.write("aa", "main\n");
    repo.commit_all("main");
    failing_merge(&repo, "side");

    let s = repo.snapshot();
    let expect = [
        ("uu", "UU", ConflictKind::BothModified),
        ("ud", "UD", ConflictKind::DeletedByThem),
        ("du", "DU", ConflictKind::DeletedByUs),
        ("aa", "AA", ConflictKind::BothAdded),
    ];
    for (path, code, kind) in expect {
        let f = find(&s, path);
        assert_eq!(xy(f), code, "{path}");
        assert_eq!(f.conflict, Some(kind), "{path}");
        assert!(
            !f.is_deleted(),
            "conflicts are reported as conflicts, not deletions"
        );
    }
    assert_eq!(s.status.conflicted_count(), 4);
    assert_eq!(
        s.status.files.len(),
        4,
        "the unchanged file is not reported"
    );
}

#[test]
fn rename_rename_conflict_reports_dd_au_ua() {
    let tmp = TempDir::new("state-rename-conflict");
    let repo = Repo::init(tmp.join("repo"));
    let body: String = (1..=50).map(|i| format!("{i}\n")).collect();
    repo.write("orig", &body);
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.git(&["mv", "orig", "theirs-name"]);
    repo.commit_all("side rename");
    repo.git(&["switch", "-q", "main"]);
    repo.git(&["mv", "orig", "our-name"]);
    repo.commit_all("main rename");
    failing_merge(&repo, "side");

    let s = repo.snapshot();
    assert_eq!(find(&s, "orig").conflict, Some(ConflictKind::BothDeleted));
    assert_eq!(find(&s, "our-name").conflict, Some(ConflictKind::AddedByUs));
    assert_eq!(
        find(&s, "theirs-name").conflict,
        Some(ConflictKind::AddedByThem)
    );
    assert_eq!(s.status.conflicted_count(), 3);
}

#[test]
fn rebase_conflict_is_detached_with_conflicts() {
    let r = Remote::new("state-rebase");
    r.work.write("file.txt", "local\n");
    r.work.commit_all("local");
    r.other.write("file.txt", "remote\n");
    r.other.commit_all("remote");
    r.other.git(&["push", "-q"]);
    r.work.git(&["fetch", "-q"]);
    let out = r.work.git_output(&["rebase", "-q", "origin/main"]);
    assert!(!out.status.success(), "rebase was expected to conflict");

    let s = r.work.snapshot();
    assert_eq!(s.branch.head, HeadState::Detached);
    assert_eq!(s.upstream, UpstreamInfo::NotApplicable);
    assert_eq!(
        find(&s, "file.txt").conflict,
        Some(ConflictKind::BothModified)
    );
    r.work.git(&["rebase", "--abort"]);
    assert_eq!(r.work.snapshot().branch.name(), Some("main"));
}

#[test]
fn snapshot_combines_every_layer_coherently() {
    let r = Remote::new("state-snapshot");
    let w = &r.work;
    w.write("staged.txt", "s\n");
    w.write("partial.txt", "p\n");
    w.write("unstaged.txt", "u\n");
    w.write("gone.txt", "g\n");
    w.commit_all("files");
    w.git(&["push", "-q"]);
    w.empty_commit("ahead 1");
    w.empty_commit("ahead 2");
    r.publish_remote_commits(1);
    w.git(&["fetch", "-q"]);

    w.write("staged.txt", "s2\n");
    w.write("partial.txt", "p2\n");
    w.git(&["add", "staged.txt", "partial.txt"]);
    w.write("partial.txt", "p3\n");
    w.write("unstaged.txt", "u2\n");
    w.remove("gone.txt");
    w.write("new/untracked.txt", "n\n");
    std::fs::create_dir_all(w.path.join("sub")).unwrap();

    let s = support::inspect(&w.path.join("sub"));
    assert_eq!(s.repository.root, w.path);
    assert_eq!(s.repository.current_directory, w.path.join("sub"));
    assert_eq!(s.repository.kind, RepositoryKind::Standard);
    assert_eq!(s.branch.name(), Some("main"));
    assert_eq!(s.branch.head_commit.as_ref().unwrap().as_str(), w.head());
    assert_eq!(s.upstream.name(), Some("origin/main"));
    assert_eq!(s.upstream.commit().unwrap().as_str(), w.rev("origin/main"));
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Compared {
            ahead: 2,
            behind: 1
        }
    );
    assert_eq!(xy(find(&s, "staged.txt")), "M.");
    assert_eq!(xy(find(&s, "partial.txt")), "MM");
    assert_eq!(xy(find(&s, "unstaged.txt")), ".M");
    assert_eq!(xy(find(&s, "gone.txt")), ".D");
    assert!(find(&s, "new/untracked.txt").is_untracked());
    assert_eq!(
        (
            s.status.staged_count(),
            s.status.unstaged_count(),
            s.status.partially_staged_count(),
            s.status.untracked_count(),
            s.status.deleted_count(),
        ),
        (2, 3, 1, 1, 1)
    );
    // Upstream tracking does not alter status: the status layer alone
    // yields the same files and counts.
    assert_eq!(status::read(&w.path).unwrap().status, s.status);
}

#[test]
fn empty_repository_snapshot() {
    let tmp = TempDir::new("state-empty");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("staged.txt", "s");
    repo.git(&["add", "staged.txt"]);
    repo.write("untracked.txt", "u");
    let s = repo.snapshot();
    assert_eq!(s.branch.head, HeadState::Unborn("main".into()));
    assert_eq!(s.branch.empty_repository, Some(true));
    assert_eq!(s.branch.head_commit, None);
    assert_eq!(s.upstream, UpstreamInfo::NotConfigured);
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream)
    );
    assert_eq!(xy(find(&s, "staged.txt")), "A.");
    assert!(find(&s, "untracked.txt").is_untracked());
}
