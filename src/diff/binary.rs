//! Binary classification and size metadata. Binary content is never decoded
//! or placed in line models.

use crate::{
    diff::{commands, errors::Result},
    types::diff::{BinarySummary, DiffFile},
};
use crate::{git, types::repository::ObjectId};
use std::path::Path;

/// Git's heuristic (`buffer_is_binary`): a NUL byte within the first 8000
/// bytes. Used only for untracked files, which Git never classified.
pub const SNIFF_BYTES: usize = 8000;

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0)
}

/// Object size from `git cat-file -s`; `None` if Git cannot report it.
pub fn blob_size(root: &Path, oid: &ObjectId) -> Result<Option<u64>> {
    let out = git::commands::output(root, &commands::blob_size(oid))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(
        std::str::from_utf8(crate::helpers::git_text::strip_newline(&out.stdout))
            .ok()
            .and_then(|s| s.parse().ok()),
    )
}

/// Sizes for a binary change: repository sides from the object database, a
/// working-tree side from the file on disk (as recorded at discovery and
/// verified unchanged).
pub fn summary(
    root: &Path,
    file: &DiffFile,
    old_oid: Option<&ObjectId>,
    new_oid: Option<&ObjectId>,
) -> Result<BinarySummary> {
    let old_size = match old_oid {
        Some(o) => blob_size(root, o)?,
        None => None,
    };
    let new_size = if file.comparison.new_side_is_working_tree() {
        file.worktree_stamp.as_ref().map(|s| s.len())
    } else {
        match new_oid {
            Some(o) => blob_size(root, o)?,
            None => None,
        }
    };
    Ok(BinarySummary { old_size, new_size })
}
