//! Ahead/behind counts from the `branch.ab` header, and when tracking is
//! unavailable and why.

use diffkit::{
    git::{commits::*, status::BranchHeaders},
    types::repository::*,
};

fn branch(head: HeadState) -> BranchInfo {
    BranchInfo {
        head,
        head_commit: None,
        empty_repository: None,
    }
}

fn available() -> UpstreamInfo {
    UpstreamInfo::Available {
        name: "origin/main".into(),
        full_ref: None,
        commit: ObjectId::parse(&[b'a'; 40]).unwrap(),
    }
}

#[test]
fn parses_counts() {
    assert_eq!(parse_ab(b"+0 -0").unwrap(), Some((0, 0)));
    assert_eq!(parse_ab(b"+12 -345").unwrap(), Some((12, 345)));
    assert_eq!(parse_ab(b"+? -?").unwrap(), None);
    for bad in [
        &b"+1"[..],
        b"1 -2",
        b"+1 2",
        b"+-1 -2",
        b"+ -",
        b"+1 -2 -3",
        b"+a -b",
    ] {
        assert!(parse_ab(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn counts_require_an_available_upstream() {
    let h = BranchHeaders {
        ab: Some(b"+2 -3".to_vec()),
        ..Default::default()
    };
    let main = branch(HeadState::Branch("main".into()));
    assert_eq!(
        tracking(&h, &main, &available()).unwrap(),
        CommitTrackingInfo::Compared {
            ahead: 2,
            behind: 3
        }
    );
    let cases = [
        (UpstreamInfo::NotConfigured, TrackingUnavailable::NoUpstream),
        (
            UpstreamInfo::NotApplicable,
            TrackingUnavailable::DetachedHead,
        ),
        (
            UpstreamInfo::Missing {
                name: "origin/x".into(),
            },
            TrackingUnavailable::UpstreamMissing,
        ),
    ];
    for (up, why) in cases {
        assert_eq!(
            tracking(&h, &main, &up).unwrap(),
            CommitTrackingInfo::Unavailable(why)
        );
    }
    let unborn = branch(HeadState::Unborn("main".into()));
    assert_eq!(
        tracking(&BranchHeaders::default(), &unborn, &available()).unwrap(),
        CommitTrackingInfo::Unavailable(TrackingUnavailable::UnbornHead)
    );
    assert_eq!(
        tracking(&BranchHeaders::default(), &main, &available()).unwrap(),
        CommitTrackingInfo::Unavailable(TrackingUnavailable::NotReported)
    );
}
