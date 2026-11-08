//! Ahead/behind counts against local bare remotes, checked against
//! `git rev-list --left-right --count HEAD...@{upstream}`.

use crate::support;

use diffkit::types::repository::{CommitTrackingInfo, TrackingRelation, TrackingUnavailable};
use support::{Remote, Repo};

fn assert_counts(r: &Remote, ahead: u64, behind: u64, relation: TrackingRelation) {
    let s = r.work.snapshot();
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Compared { ahead, behind }
    );
    assert_eq!(s.commit_tracking.relation(), Some(relation));
    assert_eq!(
        r.work.rev_list_counts("@{upstream}"),
        (ahead, behind),
        "rev-list agrees"
    );
}

#[test]
fn identical_histories() {
    let r = Remote::new("track-identical");
    assert_counts(&r, 0, 0, TrackingRelation::UpToDate);
}

#[test]
fn ahead_only_single_and_multiple() {
    let r = Remote::new("track-ahead");
    r.work.empty_commit("local 1");
    assert_counts(&r, 1, 0, TrackingRelation::Ahead);
    r.work.empty_commit("local 2");
    r.work.empty_commit("local 3");
    assert_counts(&r, 3, 0, TrackingRelation::Ahead);
}

#[test]
fn behind_only_single_and_multiple() {
    let r = Remote::new("track-behind");
    r.publish_remote_commits(1);
    r.work.git(&["fetch", "-q", "origin"]);
    assert_counts(&r, 0, 1, TrackingRelation::Behind);
    r.publish_remote_commits(3);
    r.work.git(&["fetch", "-q", "origin"]);
    assert_counts(&r, 0, 4, TrackingRelation::Behind);
}

#[test]
fn diverged_histories() {
    let r = Remote::new("track-diverged");
    r.publish_remote_commits(3);
    r.work.empty_commit("local 1");
    r.work.empty_commit("local 2");
    r.work.git(&["fetch", "-q", "origin"]);
    assert_counts(&r, 2, 3, TrackingRelation::Diverged);
}

#[test]
fn merge_commit_counts_by_reachability() {
    let r = Remote::new("track-merge");
    r.publish_remote_commits(2);
    r.work.empty_commit("local 1");
    r.work.git(&["fetch", "-q", "origin"]);
    r.work.git(&["merge", "-q", "--no-edit", "origin/main"]);
    // local commit + merge commit; the remote commits are now reachable.
    assert_counts(&r, 2, 0, TrackingRelation::Ahead);
}

#[test]
fn counts_ignore_commit_timestamps() {
    let r = Remote::new("track-dates");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&r.work.path)
        .args(["commit", "-q", "--allow-empty", "-m", "backdated"])
        .env("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_counts(&r, 1, 0, TrackingRelation::Ahead);
}

#[test]
fn no_upstream_is_unavailable_not_zero() {
    let r = Remote::new("track-no-upstream");
    r.work.git(&["switch", "-q", "-c", "untracked-branch"]);
    let t = r.work.snapshot().commit_tracking;
    assert_eq!(
        t,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream)
    );
    assert_eq!((t.ahead(), t.behind(), t.relation()), (None, None, None));
    assert!(!t.comparison_available());
}

#[test]
fn missing_upstream_ref_is_unavailable() {
    let r = Remote::new("track-missing");
    r.work
        .git(&["config", "branch.main.merge", "refs/heads/gone"]);
    assert_eq!(
        r.work.snapshot().commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::UpstreamMissing)
    );
}

#[test]
fn stale_tracking_ref_is_reported_as_locally_known_and_never_fetched() {
    let r = Remote::new("track-stale");
    let known = r.work.rev("refs/remotes/origin/main");
    r.publish_remote_commits(2);
    let actual_remote = r.bare.rev("refs/heads/main");
    assert_ne!(known, actual_remote);

    // Without a fetch, Diffkit can only see the stale local tracking ref.
    let s = r.work.snapshot();
    assert_eq!(s.upstream.commit().unwrap().as_str(), known);
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Compared {
            ahead: 0,
            behind: 0
        }
    );
    // ...and inspecting did not fetch.
    assert_eq!(r.work.rev("refs/remotes/origin/main"), known);

    r.work.git(&["fetch", "-q", "origin"]);
    let s = r.work.snapshot();
    assert_eq!(s.upstream.commit().unwrap().as_str(), actual_remote);
    assert_counts(&r, 0, 2, TrackingRelation::Behind);
}

#[test]
fn deleted_upstream_is_unavailable() {
    let r = Remote::new("track-deleted");
    r.work.git(&["switch", "-q", "-c", "feature"]);
    r.work.git(&["push", "-q", "-u", "origin", "feature"]);
    r.work.empty_commit("ahead before deletion");
    assert_counts(&r, 1, 0, TrackingRelation::Ahead);
    r.other
        .git(&["push", "-q", "origin", "--delete", "feature"]);
    r.work.git(&["fetch", "-q", "--prune"]);
    assert_eq!(
        r.work.snapshot().commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::UpstreamMissing)
    );
}

#[test]
fn detached_head_is_unavailable() {
    let r = Remote::new("track-detached");
    r.work.empty_commit("local");
    r.work.git(&["checkout", "-q", "--detach"]);
    assert_eq!(
        r.work.snapshot().commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::DetachedHead)
    );
}

#[test]
fn shallow_clone_tracking() {
    let r = Remote::new("track-shallow");
    r.publish_remote_commits(3);
    let url = format!("file://{}", r.bare.path.display());
    support::git(
        r.tmp.path(),
        &["clone", "-q", "--depth", "1", &url, "shallow"],
    );
    let shallow = Repo {
        path: r.tmp.join("shallow"),
    };
    shallow.git(&["config", "user.name", "Diffkit Test"]);
    shallow.git(&["config", "user.email", "test@diffkit.invalid"]);
    shallow.empty_commit("local on shallow");
    let s = shallow.snapshot();
    assert!(s.repository.shallow);
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Compared {
            ahead: 1,
            behind: 0
        }
    );
    assert_eq!(shallow.rev_list_counts("@{upstream}"), (1, 0));
}
