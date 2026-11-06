//! Ahead/behind tracking against the locally known upstream.
//!
//! The counts come from the `# branch.ab +<ahead> -<behind>` header of the
//! status call. Git computes it the same way as
//! `git rev-list --left-right --count HEAD...@{upstream}`: by commit-graph
//! reachability, never by timestamps or messages. Reusing it avoids a
//! separate subprocess.

use super::{
    errors::{GitError, Result},
    models::{BranchInfo, CommitTrackingInfo, TrackingUnavailable, UpstreamInfo},
    status::BranchHeaders,
};

pub fn tracking(
    h: &BranchHeaders,
    branch: &BranchInfo,
    upstream: &UpstreamInfo,
) -> Result<CommitTrackingInfo> {
    use TrackingUnavailable::*;
    let unavailable = |why| Ok(CommitTrackingInfo::Unavailable(why));
    match upstream {
        UpstreamInfo::NotApplicable => return unavailable(DetachedHead),
        UpstreamInfo::NotConfigured => return unavailable(NoUpstream),
        UpstreamInfo::Missing { .. } => return unavailable(UpstreamMissing),
        UpstreamInfo::Available { .. } if branch.unborn() => return unavailable(UnbornHead),
        UpstreamInfo::Available { .. } => {}
    }
    match h.ab.as_deref().map(parse_ab).transpose()? {
        Some(Some((ahead, behind))) => Ok(CommitTrackingInfo::Compared { ahead, behind }),
        Some(None) | None => unavailable(NotReported),
    }
}

/// Parses `+<ahead> -<behind>`. `+? -?` (counts not computed) is `None`.
pub fn parse_ab(value: &[u8]) -> Result<Option<(u64, u64)>> {
    if value == b"+? -?" {
        return Ok(None);
    }
    let bad = || GitError::malformed("branch.ab", String::from_utf8_lossy(value).into_owned());
    let s = std::str::from_utf8(value).map_err(|_| bad())?;
    let (a, b) = s.split_once(' ').ok_or_else(bad)?;
    let num = |part: &str, sign: char| -> Result<u64> {
        part.strip_prefix(sign)
            .filter(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|d| d.parse().ok())
            .ok_or_else(bad)
    };
    Ok(Some((num(a, '+')?, num(b, '-')?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::models::{HeadState, ObjectId};

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
}
