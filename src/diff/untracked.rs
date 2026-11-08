//! Untracked files: listed by `git ls-files --others --exclude-standard -z`
//! (so Git applies every ignore rule), read directly from the working tree.
//! Nothing is staged, hashed into the object database, or written.
//!
//! Safety rules when reading:
//! - Paths must be relative and contain only normal components.
//! - The parent directory must resolve inside the repository, so a directory
//!   swapped for a symlink cannot redirect the read.
//! - The file itself is inspected with `lstat`; symlinks are not followed,
//!   their target is the content (as Git stores it).
//! - The opened file must be the same file that was `lstat`ed, and its
//!   metadata must not change while it is read; otherwise the result is
//!   [`DiffError::Stale`].
//! - Files larger than the configured limit are reported as
//!   [`DiffContent::TooLarge`] without being read.

use crate::git::{self, paths::path_from_git};
use crate::{
    diff::{
        binary, commands,
        errors::{DiffError, Result, StaleReason},
        hunk,
    },
    types::diff::{
        BinaryStatus, BinarySummary, ContentSource, DiffComparison, DiffContent, DiffFile,
        FileChangeKind, FileDiff, FileId, FileMode, StampType, WorktreeStamp,
    },
};
use std::{
    fs, io,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub fn stamp(path: &Path) -> io::Result<WorktreeStamp> {
    let m = fs::symlink_metadata(path)?;
    Ok(stamp_of(&m))
}

fn stamp_of(m: &fs::Metadata) -> WorktreeStamp {
    let ft = m.file_type();
    WorktreeStamp {
        len: m.len(),
        modified: m.modified().ok(),
        file_type: if ft.is_symlink() {
            StampType::Symlink
        } else if ft.is_dir() {
            StampType::Directory
        } else if ft.is_file() {
            StampType::File
        } else {
            StampType::Other
        },
        #[cfg(unix)]
        inode: std::os::unix::fs::MetadataExt::ino(m),
        #[cfg(unix)]
        mode: std::os::unix::fs::MetadataExt::mode(m),
    }
}

fn mode_of(s: &WorktreeStamp) -> Option<FileMode> {
    match s.file_type {
        StampType::Symlink => Some(FileMode(0o120000)),
        StampType::File => {
            #[cfg(unix)]
            let exec = s.mode & 0o111 != 0;
            #[cfg(not(unix))]
            let exec = false;
            Some(FileMode(if exec { 0o100755 } else { 0o100644 }))
        }
        StampType::Directory | StampType::Other => None,
    }
}

pub fn discover(root: &Path) -> Result<Vec<DiffFile>> {
    let out = git::commands::run(root, &commands::UNTRACKED)?;
    let Some(body) = out.strip_suffix(b"\0") else {
        return if out.is_empty() {
            Ok(Vec::new())
        } else {
            Err(DiffError::parse(0, "ls-files output is not NUL-terminated"))
        };
    };
    from_git_paths(root, body.split(|&b| b == 0))
}

/// Builds untracked entries from paths Git listed, relative to the root:
/// `ls-files --others` output, or the `?` records of a status taken with
/// `--untracked-files=all` (the same list, from the same ignore rules).
pub fn from_git_paths<B: AsRef<[u8]>>(
    root: &Path,
    paths: impl IntoIterator<Item = B>,
) -> Result<Vec<DiffFile>> {
    let mut files = Vec::new();
    for raw in paths {
        let raw = raw.as_ref();
        // Nested repositories are listed as "dir/".
        let raw = raw.strip_suffix(b"/").unwrap_or(raw);
        if raw.is_empty() {
            return Err(DiffError::parse(0, "empty untracked path from Git"));
        }
        let path = path_from_git(raw)?;
        let worktree_stamp = stamp(&root.join(&path)).ok();
        files.push(DiffFile {
            id: FileId(files.len()),
            comparison: DiffComparison::Untracked,
            old_path: None,
            new_path: Some(path),
            kind: FileChangeKind::Added,
            old_mode: None,
            new_mode: worktree_stamp.as_ref().and_then(mode_of),
            similarity: None,
            old_oid: None,
            new_oid: None,
            binary: BinaryStatus::Unknown,
            stats: None,
            conflicted: false,
            worktree_stamp,
        });
    }
    Ok(files)
}

/// Resolves `rel` under `root`, refusing anything that could leave it.
fn resolve(root: &Path, rel: &Path) -> Result<PathBuf> {
    let outside = || DiffError::OutsideRepository {
        path: rel.to_path_buf(),
    };
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(outside());
    }
    let full = root.join(rel);
    let canon_root = root.canonicalize().map_err(|source| DiffError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if let Some(parent) = full.parent() {
        match parent.canonicalize() {
            Ok(p) if p.starts_with(&canon_root) => {}
            Ok(_) => return Err(outside()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(DiffError::Vanished {
                    path: rel.to_path_buf(),
                })
            }
            Err(source) => return Err(DiffError::Io { path: full, source }),
        }
    }
    Ok(full)
}

pub fn load(root: &Path, file: &DiffFile, max_bytes: u64) -> Result<FileDiff> {
    let rel = file
        .new_path
        .as_deref()
        .expect("untracked entries have a path");
    let full = resolve(root, rel)?;
    let vanished = || DiffError::Vanished {
        path: rel.to_path_buf(),
    };
    let stale = || DiffError::Stale {
        path: rel.to_path_buf(),
        reason: StaleReason::WorkingTreeChanged,
    };
    let before = match stamp(&full) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(vanished()),
        Err(source) => return Err(DiffError::Io { path: full, source }),
    };
    if file.worktree_stamp.as_ref() != Some(&before) {
        return Err(stale());
    }
    let mut meta = file.clone();
    let done = |meta: DiffFile, content| FileDiff {
        file: meta,
        content,
        source: ContentSource::WorkingTree,
        old_oid: None,
        new_oid: None,
    };
    let bytes = match before.file_type {
        StampType::Directory => {
            let content = if full.join(".git").exists() {
                DiffContent::NestedRepository
            } else {
                DiffContent::Unreadable {
                    reason: "is a directory".into(),
                }
            };
            return Ok(done(meta, content));
        }
        StampType::Other => {
            return Ok(done(
                meta,
                DiffContent::Unreadable {
                    reason: "not a regular file or symlink".into(),
                },
            ))
        }
        StampType::Symlink => {
            let target = fs::read_link(&full).map_err(|source| DiffError::Io {
                path: full.clone(),
                source,
            })?;
            git::paths::path_bytes(&target).into_owned()
        }
        StampType::File => {
            if before.len > max_bytes {
                return Ok(done(
                    meta,
                    DiffContent::TooLarge {
                        limit: max_bytes,
                        size: Some(before.len),
                    },
                ));
            }
            let mut f = match fs::File::open(&full) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(vanished()),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    return Ok(done(
                        meta,
                        DiffContent::Unreadable {
                            reason: e.to_string(),
                        },
                    ))
                }
                Err(source) => return Err(DiffError::Io { path: full, source }),
            };
            let opened = f.metadata().map_err(|source| DiffError::Io {
                path: full.clone(),
                source,
            })?;
            // The open followed whatever is at the path now; it must still
            // be the regular file that was inspected.
            if stamp_of(&opened) != before {
                return Err(stale());
            }
            let mut buf = Vec::with_capacity(before.len as usize);
            (&mut f)
                .take(max_bytes + 1)
                .read_to_end(&mut buf)
                .map_err(|source| DiffError::Io {
                    path: full.clone(),
                    source,
                })?;
            if buf.len() as u64 > max_bytes {
                return Ok(done(
                    meta,
                    DiffContent::TooLarge {
                        limit: max_bytes,
                        size: None,
                    },
                ));
            }
            buf
        }
    };
    match stamp(&full) {
        Ok(after) if after == before => {}
        Ok(_) => return Err(stale()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(vanished()),
        Err(source) => return Err(DiffError::Io { path: full, source }),
    }
    if binary::looks_binary(&bytes) {
        meta.binary = BinaryStatus::Binary;
        return Ok(done(
            meta,
            DiffContent::Binary(BinarySummary {
                old_size: None,
                new_size: Some(bytes.len() as u64),
            }),
        ));
    }
    meta.binary = BinaryStatus::Text;
    let lines = hunk::split_lines(&bytes);
    drop(bytes);
    let hunks = hunk::addition_hunk(lines).into_iter().collect();
    Ok(done(meta, DiffContent::Text { hunks }))
}
