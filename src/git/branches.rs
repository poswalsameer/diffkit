//! Branch, HEAD, and upstream resolution.
//!
//! The current branch and HEAD commit come from the `# branch.*` headers of
//! the status call. Git is consulted again only to resolve the upstream
//! commit and, for an unborn HEAD, to tell an empty repository apart from
//! an orphan branch.

use crate::{
    git::{
        commands,
        errors::{GitError, Result},
        status::BranchHeaders,
    },
    helpers::git_text::strip_newline,
    types::repository::{BranchInfo, HeadState, ObjectId, UpstreamInfo},
};
use std::path::Path;

/// Interprets `branch.oid` and `branch.head` without running Git.
pub fn from_headers(h: &BranchHeaders) -> Result<BranchInfo> {
    let oid = h
        .oid
        .as_deref()
        .ok_or_else(|| GitError::malformed("status branch headers", "missing branch.oid"))?;
    let head_commit = match oid {
        b"(initial)" => None,
        _ => Some(ObjectId::parse(oid).ok_or_else(|| {
            GitError::malformed("branch.oid", String::from_utf8_lossy(oid).into_owned())
        })?),
    };
    let head = match h.head.as_deref() {
        Some(b"(detached)") => HeadState::Detached,
        Some(b"(unknown)") | None => HeadState::Unknown,
        Some(name) => {
            // Ref names are bytes; Git forbids control characters but not
            // non-UTF-8 sequences, which are displayed with replacement.
            let name = String::from_utf8_lossy(name).into_owned();
            if head_commit.is_some() {
                HeadState::Branch(name)
            } else {
                HeadState::Unborn(name)
            }
        }
    };
    Ok(BranchInfo {
        head,
        head_commit,
        empty_repository: None,
    })
}

/// Builds [`BranchInfo`] from the status headers. For an unborn HEAD, one
/// `for-each-ref` call checks whether the repository has any refs at all.
pub fn resolve(root: &Path, h: &BranchHeaders) -> Result<BranchInfo> {
    let mut info = from_headers(h)?;
    if info.unborn() {
        let refs = commands::run(root, &["for-each-ref", "--count=1", "--format=%(refname)"])?;
        info.empty_repository = Some(refs.is_empty());
    }
    Ok(info)
}

/// Resolves the configured upstream against *local* refs only.
///
/// - No `branch.upstream` header: nothing configured (or HEAD is detached).
/// - Header plus `branch.ab`: Git resolved the tracking ref, so one
///   `rev-parse` returns its commit and full name.
/// - Header without `branch.ab` on a born branch: Git could not resolve the
///   tracking ref, so it is missing; no extra call is needed.
/// - Unborn HEAD: Git never prints `branch.ab`, so the ref is checked with
///   `rev-parse --verify --quiet`, which exits 1 when it does not exist.
pub fn resolve_upstream(
    root: &Path,
    h: &BranchHeaders,
    branch: &BranchInfo,
) -> Result<UpstreamInfo> {
    let Some(raw) = h.upstream.as_deref() else {
        return Ok(match branch.head {
            HeadState::Detached | HeadState::Unknown => UpstreamInfo::NotApplicable,
            _ => UpstreamInfo::NotConfigured,
        });
    };
    let name = String::from_utf8_lossy(raw).into_owned();
    if branch.unborn() {
        let args = ["rev-parse", "--verify", "--quiet", "@{upstream}"];
        let out = commands::output(root, &args)?;
        return match out.status.code() {
            Some(0) => Ok(UpstreamInfo::Available {
                name,
                full_ref: None,
                commit: oid_line(&out.stdout)?,
            }),
            Some(1) => Ok(UpstreamInfo::Missing { name }),
            _ => Err(commands::failure(&args, &out)),
        };
    }
    if h.ab.is_none() {
        return Ok(UpstreamInfo::Missing { name });
    }
    let out = commands::run(
        root,
        &[
            "rev-parse",
            "@{upstream}",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    let mut lines = strip_newline(&out).split(|&b| b == b'\n');
    let commit = oid_line(lines.next().unwrap_or_default())?;
    let full_ref = lines
        .next()
        .filter(|l| l.starts_with(b"refs/"))
        .map(|l| String::from_utf8_lossy(l).into_owned());
    Ok(UpstreamInfo::Available {
        name,
        full_ref,
        commit,
    })
}

fn oid_line(bytes: &[u8]) -> Result<ObjectId> {
    let line = strip_newline(bytes);
    ObjectId::parse(line).ok_or_else(|| {
        GitError::malformed(
            "upstream commit",
            String::from_utf8_lossy(line).into_owned(),
        )
    })
}
