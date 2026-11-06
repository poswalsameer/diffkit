//! Upstream resolution against local bare remotes. No network is used.

mod common;

use common::{Remote, Repo, TempDir};
use diffkit::git::models::{CommitTrackingInfo, TrackingUnavailable, UpstreamInfo};

#[test]
fn configured_upstream_with_existing_ref() {
    let r = Remote::new("upstream-configured");
    let s = r.work.snapshot();
    assert_eq!(
        s.upstream,
        UpstreamInfo::Available {
            name: "origin/main".into(),
            full_ref: Some("refs/remotes/origin/main".into()),
            commit: s.branch.head_commit.clone().unwrap(),
        }
    );
    assert!(s.upstream.configured() && s.upstream.available());
    assert_eq!(
        s.upstream.commit().unwrap().as_str(),
        r.work.rev("refs/remotes/origin/main")
    );
}

#[test]
fn branch_with_slashes_tracks_matching_remote_branch() {
    let r = Remote::new("upstream-slash");
    r.work.git(&["switch", "-q", "-c", "feature/login"]);
    r.work.git(&["push", "-q", "-u", "origin", "feature/login"]);
    let s = r.work.snapshot();
    assert_eq!(s.upstream.name(), Some("origin/feature/login"));
    match &s.upstream {
        UpstreamInfo::Available { full_ref, .. } => {
            assert_eq!(
                full_ref.as_deref(),
                Some("refs/remotes/origin/feature/login")
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn branch_without_upstream_while_remote_exists() {
    let r = Remote::new("upstream-none");
    r.work.git(&["switch", "-q", "-c", "local-only"]);
    let s = r.work.snapshot();
    assert_eq!(s.upstream, UpstreamInfo::NotConfigured);
    assert!(!s.upstream.configured());
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream)
    );
}

#[test]
fn repository_without_any_remote() {
    let tmp = TempDir::new("upstream-no-remote");
    let repo = Repo::init(tmp.join("repo"));
    repo.empty_commit("c");
    assert!(repo.git(&["remote"]).trim().is_empty());
    let s = repo.snapshot();
    assert_eq!(s.upstream, UpstreamInfo::NotConfigured);
    assert_eq!(s.commit_tracking.ahead(), None);
}

#[test]
fn configured_upstream_whose_ref_was_never_fetched() {
    let r = Remote::new("upstream-missing");
    r.work
        .git(&["config", "branch.main.merge", "refs/heads/does-not-exist"]);
    let s = r.work.snapshot();
    assert_eq!(
        s.upstream,
        UpstreamInfo::Missing {
            name: "origin/does-not-exist".into()
        }
    );
    assert!(s.upstream.configured() && !s.upstream.available());
    assert_eq!(s.upstream.commit(), None);
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::UpstreamMissing)
    );
}

#[test]
fn upstream_branch_deleted_on_remote_and_pruned() {
    let r = Remote::new("upstream-deleted");
    r.work.git(&["switch", "-q", "-c", "feature"]);
    r.work.empty_commit("feature work");
    r.work.git(&["push", "-q", "-u", "origin", "feature"]);
    assert!(r.work.snapshot().upstream.available());

    r.other
        .git(&["push", "-q", "origin", "--delete", "feature"]);
    r.work.git(&["fetch", "-q", "--prune", "origin"]);
    let s = r.work.snapshot();
    assert_eq!(
        s.upstream,
        UpstreamInfo::Missing {
            name: "origin/feature".into()
        }
    );
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::UpstreamMissing)
    );
}

#[test]
fn upstream_can_be_a_local_branch() {
    let r = Remote::new("upstream-local");
    r.work.git(&["switch", "-q", "-c", "topic"]);
    r.work.git(&["branch", "-q", "--set-upstream-to=main"]);
    r.work.empty_commit("topic work");
    let s = r.work.snapshot();
    match &s.upstream {
        UpstreamInfo::Available {
            name,
            full_ref,
            commit,
        } => {
            assert_eq!(name, "main");
            assert_eq!(full_ref.as_deref(), Some("refs/heads/main"));
            assert_eq!(commit.as_str(), r.work.rev("main"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Compared {
            ahead: 1,
            behind: 0
        }
    );
}

#[test]
fn detached_head_has_no_branch_upstream() {
    let r = Remote::new("upstream-detached");
    r.work.git(&["checkout", "-q", "--detach"]);
    let s = r.work.snapshot();
    assert_eq!(s.upstream, UpstreamInfo::NotApplicable);
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::DetachedHead)
    );
}

#[test]
fn unborn_branch_with_configured_upstream() {
    let r = Remote::new("upstream-unborn");
    r.work.git(&["switch", "-q", "--orphan", "fresh"]);
    r.work.git(&["config", "branch.fresh.remote", "origin"]);
    r.work
        .git(&["config", "branch.fresh.merge", "refs/heads/main"]);
    let s = r.work.snapshot();
    assert!(s.branch.unborn());
    assert_eq!(s.upstream.name(), Some("origin/main"));
    assert_eq!(
        s.upstream.commit().unwrap().as_str(),
        r.work.rev("refs/remotes/origin/main")
    );
    assert_eq!(
        s.commit_tracking,
        CommitTrackingInfo::Unavailable(TrackingUnavailable::UnbornHead)
    );

    r.work
        .git(&["config", "branch.fresh.merge", "refs/heads/nope"]);
    let s = r.work.snapshot();
    assert_eq!(
        s.upstream,
        UpstreamInfo::Missing {
            name: "origin/nope".into()
        }
    );
}
