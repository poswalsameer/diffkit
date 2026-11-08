//! Ahead/behind tracking against the locally known upstream.
//!
//! The counts come from the `# branch.ab +<ahead> -<behind>` header of the
//! status call. Git computes it the same way as
//! `git rev-list --left-right --count HEAD...@{upstream}`: by commit-graph
//! reachability, never by timestamps or messages. Reusing it avoids a
//! separate subprocess.

use crate::{
    git::{
        errors::{GitError, Result},
        status::BranchHeaders,
    },
    types::repository::{BranchInfo, CommitTrackingInfo, TrackingUnavailable, UpstreamInfo},
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
