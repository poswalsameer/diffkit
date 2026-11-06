//! Assembles one [`RepositorySnapshot`] from the individual layers.
//!
//! Git subprocesses per inspection:
//! 1. `rev-parse` for repository paths and flags (three more only if a
//!    repository path contains a newline);
//! 2. one `status --porcelain=v2 -z --branch`, which also supplies the
//!    branch, HEAD, upstream name, and ahead/behind headers;
//! 3. `rev-parse` for the upstream commit, only when an upstream is
//!    configured and its ref exists (or HEAD is unborn);
//! 4. `for-each-ref --count=1`, only when HEAD is unborn.
//!
//! The commands run one after another, so a concurrent Git operation can
//! land between them; the snapshot is consistent per command, not atomic.

use super::{branches, commits, errors::Result, models::RepositorySnapshot, repository, status};
use std::path::Path;

pub fn inspect(start: &Path) -> Result<RepositorySnapshot> {
    let repository = repository::discover_from(start)?;
    let report = status::read(&repository.root)?;
    let branch = branches::resolve(&repository.root, &report.headers)?;
    let upstream = branches::resolve_upstream(&repository.root, &report.headers, &branch)?;
    let commit_tracking = commits::tracking(&report.headers, &branch, &upstream)?;
    Ok(RepositorySnapshot {
        repository,
        branch,
        upstream,
        commit_tracking,
        status: report.status,
    })
}

pub fn inspect_current_dir() -> Result<RepositorySnapshot> {
    inspect(&repository::current_dir()?)
}
