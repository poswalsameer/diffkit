//! Public diff API.
//!
//! ```text
//! let engine = DiffEngine::open(path, DiffOptions::default())?;
//! let list = engine.discover(DiffComparison::Staged)?;      // 1 git call, no content
//! let diff = engine.load_file_diff(&list, list.files[0].id)?; // 1 git call for that file
//! let all  = engine.load_all(&list)?;                       // 1 git call for the list
//! ```
//!
//! Nothing is cached: every load asks Git (or reads the working tree)
//! again, and its result is checked against the discovered metadata. If the
//! file changed in between, loading returns [`DiffError::Stale`] rather than
//! attaching new content to old metadata.

use super::{
    binary, commands, discovery,
    errors::{DiffError, Result, StaleReason},
    hunk,
    models::{
        BinaryStatus, ContentSource, DiffCollection, DiffComparison, DiffContent, DiffFile,
        DiffFileList, DiffOptions, FileChangeKind, FileDiff, FileId, LineStats, LoadedFile,
        SubmoduleChange,
    },
    parser::{self, FilePatch, Origin, PatchKind},
    untracked,
};
use crate::git::{
    self,
    commands::{path_bytes, LimitedOutput},
    models::{ObjectFormat, ObjectId, RepositorySnapshot},
    snapshot,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct DiffEngine {
    root: PathBuf,
    head: Option<ObjectId>,
    object_format: ObjectFormat,
    conflicted: HashSet<PathBuf>,
    options: DiffOptions,
}

/// (old path, new path) as Git bytes; identifies a file section in a patch.
type PathKey = (Option<Vec<u8>>, Option<Vec<u8>>);

impl DiffEngine {
    /// Builds an engine from an existing Phase 1 snapshot. HEAD and the set
    /// of conflicted paths are taken from it; no Git command runs.
    pub fn from_snapshot(s: &RepositorySnapshot, options: DiffOptions) -> Self {
        Self {
            root: s.repository.root.clone(),
            head: s.branch.head_commit.clone(),
            object_format: s.repository.object_format,
            conflicted: s
                .status
                .files
                .iter()
                .filter(|f| f.is_conflicted())
                .map(|f| f.path.clone())
                .collect(),
            options,
        }
    }

    /// Inspects the repository containing `start` (Phase 1 snapshot) and
    /// builds an engine for it.
    pub fn open(start: &Path, options: DiffOptions) -> Result<Self> {
        Ok(Self::from_snapshot(&snapshot::inspect(start)?, options))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn options(&self) -> &DiffOptions {
        &self.options
    }

    /// The HEAD commit, or `None` when HEAD is unborn.
    pub fn head(&self) -> Option<&ObjectId> {
        self.head.as_ref()
    }

    /// The commit used as HEAD, or the empty tree when HEAD is unborn.
    pub fn base(&self) -> ObjectId {
        self.head
            .clone()
            .unwrap_or_else(|| self.object_format.empty_tree())
    }

    /// Lists changed files for one comparison without loading content.
    pub fn discover(&self, comparison: DiffComparison) -> Result<DiffFileList> {
        let (base, files, stat_only_entries) = match comparison {
            DiffComparison::Untracked => (None, untracked::discover(&self.root)?, 0),
            _ => {
                let base = self.base();
                let out = git::commands::run(&self.root, &commands::discovery(comparison, &base))?;
                let found = discovery::parse_raw_numstat(&out)?;
                let files =
                    discovery::build_files(found.pairs, comparison, &self.root, &self.conflicted)?;
                (
                    (comparison != DiffComparison::Unstaged).then_some(base),
                    files,
                    found.stat_only,
                )
            }
        };
        Ok(DiffFileList {
            comparison,
            base,
            files,
            stat_only_entries,
        })
    }

    /// Loads the diff of one discovered file.
    pub fn load_file_diff(&self, list: &DiffFileList, id: FileId) -> Result<FileDiff> {
        let file = list.get(id).ok_or(DiffError::UnknownFile(id))?;
        if file.comparison == DiffComparison::Untracked {
            return untracked::load(&self.root, file, self.options.max_untracked_bytes);
        }
        if file.kind == FileChangeKind::Unmerged {
            return Ok(unmerged(file));
        }
        self.check_worktree(file)?;
        let mut paths: Vec<&Path> = file
            .old_path
            .iter()
            .chain(&file.new_path)
            .map(PathBuf::as_path)
            .collect();
        paths.dedup();
        let base = self.base();
        let args = commands::patch(file.comparison, &base, self.options.context_lines, &paths);
        let out = match git::commands::output_limited(
            &self.root,
            &args,
            self.options.max_file_patch_bytes,
        )? {
            LimitedOutput::Complete(out) if out.status.success() => out,
            LimitedOutput::Complete(out) => return Err(git::commands::failure(&args, &out).into()),
            LimitedOutput::Exceeded { limit } => return Ok(too_large(file, limit)),
        };
        let mut sections = parser::parse_patch(&out.stdout)?;
        let wanted = section_keys(file);
        let mut matched = Vec::new();
        for key in &wanted {
            let i = sections
                .iter()
                .position(|s| &key_of(s) == key)
                .ok_or_else(|| stale(file, StaleReason::MissingFromDiff))?;
            matched.push(sections.swap_remove(i));
        }
        self.check_worktree(file)?;
        self.build(file, matched)
    }

    /// Loads every file of `list`. Tracked comparisons use a single
    /// `git diff`; if its output exceeds `max_total_patch_bytes`, files are
    /// loaded one by one instead (each bounded by `max_file_patch_bytes`).
    pub fn load_all(&self, list: &DiffFileList) -> Result<DiffCollection> {
        let one_by_one = |unexpected_paths| DiffCollection {
            comparison: list.comparison,
            files: list
                .files
                .iter()
                .map(|f| LoadedFile {
                    id: f.id,
                    result: self.load_file_diff(list, f.id),
                })
                .collect(),
            unexpected_paths,
        };
        if list.comparison == DiffComparison::Untracked || list.is_empty() {
            return Ok(one_by_one(Vec::new()));
        }
        let base = self.base();
        let args = commands::patch(list.comparison, &base, self.options.context_lines, &[]);
        let out = match git::commands::output_limited(
            &self.root,
            &args,
            self.options.max_total_patch_bytes,
        )? {
            LimitedOutput::Complete(out) if out.status.success() => out,
            LimitedOutput::Complete(out) => return Err(git::commands::failure(&args, &out).into()),
            LimitedOutput::Exceeded { .. } => return Ok(one_by_one(Vec::new())),
        };
        let mut sections: Vec<Option<FilePatch>> = parser::parse_patch(&out.stdout)?
            .into_iter()
            .map(Some)
            .collect();
        let mut by_key: HashMap<PathKey, Vec<usize>> = HashMap::new();
        for (i, s) in sections.iter().enumerate() {
            by_key
                .entry(key_of(s.as_ref().expect("fresh")))
                .or_default()
                .push(i);
        }
        let mut files = Vec::with_capacity(list.len());
        for file in &list.files {
            let mut matched = Vec::new();
            let mut missing = false;
            for key in section_keys(file) {
                match by_key
                    .get_mut(&key)
                    .and_then(|v| (!v.is_empty()).then(|| v.remove(0)))
                {
                    Some(i) => matched.push(sections[i].take().expect("each section used once")),
                    None => missing = true,
                }
            }
            let result = if file.kind == FileChangeKind::Unmerged {
                Ok(unmerged(file))
            } else if missing {
                Err(stale(file, StaleReason::MissingFromDiff))
            } else {
                self.check_worktree(file)
                    .and_then(|()| self.build(file, matched))
            };
            files.push(LoadedFile {
                id: file.id,
                result,
            });
        }
        let mut unexpected_paths = Vec::new();
        for s in sections.into_iter().flatten() {
            if let Some(p) = s.new_path.or(s.old_path) {
                unexpected_paths.push(git::commands::path_from_git(&p)?);
            }
        }
        Ok(DiffCollection {
            comparison: list.comparison,
            files,
            unexpected_paths,
        })
    }

    /// The working-tree side must still match the metadata seen at discovery.
    fn check_worktree(&self, file: &DiffFile) -> Result<()> {
        if !file.comparison.new_side_is_working_tree() {
            return Ok(());
        }
        let Some(p) = &file.new_path else {
            return Ok(());
        };
        let now = untracked::stamp(&self.root.join(p)).ok();
        if now != file.worktree_stamp {
            return Err(stale(file, StaleReason::WorkingTreeChanged));
        }
        Ok(())
    }

    fn build(&self, file: &DiffFile, sections: Vec<FilePatch>) -> Result<FileDiff> {
        if sections.iter().any(|s| s.kind != PatchKind::Normal) {
            // Git now reports a conflict that discovery did not see.
            return Err(stale(file, StaleReason::MissingFromDiff));
        }
        let first = sections.first().expect("at least one section");
        let last = sections.last().expect("at least one section");
        let (old_oid, new_oid) = (first.old_oid.clone(), last.new_oid.clone());
        let has_index_line = sections
            .iter()
            .any(|s| s.old_oid.is_some() || s.new_oid.is_some());
        if has_index_line {
            let differs =
                |seen: &Option<ObjectId>, now: &Option<ObjectId>| seen.is_some() && seen != now;
            let new_from_repo = !file.comparison.new_side_is_working_tree();
            if differs(&file.old_oid, &old_oid)
                || (new_from_repo && differs(&file.new_oid, &new_oid))
            {
                return Err(stale(file, StaleReason::ObjectChanged));
            }
        }
        let is_binary = sections.iter().any(|s| s.binary);
        if (file.binary == BinaryStatus::Binary) != is_binary {
            return Err(stale(file, StaleReason::BinaryChanged));
        }
        let (added, removed) = sections.iter().fold((0, 0), |(a, r), s| {
            let (sa, sr) = s.counts();
            (a + sa, r + sr)
        });
        if !is_binary
            && file
                .stats
                .is_some_and(|s| s != LineStats { added, removed })
        {
            return Err(stale(file, StaleReason::StatsChanged));
        }
        let submodule = [file.old_mode, file.new_mode]
            .iter()
            .flatten()
            .any(|m| m.0 == 0o160000);
        let content = if submodule {
            DiffContent::Submodule(submodule_change(file, &sections))
        } else if is_binary {
            DiffContent::Binary(binary::summary(
                &self.root,
                file,
                file.old_oid.as_ref().or(old_oid.as_ref()),
                file.new_oid.as_ref().or(new_oid.as_ref()),
            )?)
        } else if sections.len() == 1 {
            let hunks = hunk::build_hunks(&first.hunks, &self.options.alignment)?;
            DiffContent::Text { hunks }
        } else {
            // A type change is printed as a deletion and an addition; present
            // it as one hunk from the old content to the new content.
            DiffContent::Text {
                hunks: type_change_hunk(&sections, &self.options.alignment)?
                    .into_iter()
                    .collect(),
            }
        };
        Ok(FileDiff {
            file: file.clone(),
            content,
            source: ContentSource::Git,
            old_oid,
            new_oid,
        })
    }
}

fn stale(file: &DiffFile, reason: StaleReason) -> DiffError {
    DiffError::Stale {
        path: file.path().to_path_buf(),
        reason,
    }
}

fn unmerged(file: &DiffFile) -> FileDiff {
    FileDiff {
        file: file.clone(),
        content: DiffContent::Unmerged,
        source: ContentSource::Metadata,
        old_oid: None,
        new_oid: None,
    }
}

fn too_large(file: &DiffFile, limit: u64) -> FileDiff {
    FileDiff {
        file: file.clone(),
        content: DiffContent::TooLarge { limit, size: None },
        source: ContentSource::Git,
        old_oid: None,
        new_oid: None,
    }
}

fn key_of(s: &FilePatch) -> PathKey {
    (s.old_path.clone(), s.new_path.clone())
}

fn bytes(p: &Option<PathBuf>) -> Option<Vec<u8>> {
    p.as_deref().map(|p| path_bytes(p).into_owned())
}

/// The patch sections a file corresponds to. A type change appears as two
/// sections: the old path deleted and the new path added.
fn section_keys(file: &DiffFile) -> Vec<PathKey> {
    let (old, new) = (bytes(&file.old_path), bytes(&file.new_path));
    match file.kind {
        FileChangeKind::TypeChanged => vec![(old, None), (None, new)],
        _ => vec![(old, new)],
    }
}

fn submodule_change(file: &DiffFile, sections: &[FilePatch]) -> SubmoduleChange {
    let mut new_dirty = false;
    for line in sections
        .iter()
        .flat_map(|s| &s.hunks)
        .flat_map(|h| &h.lines)
    {
        if line.origin == Origin::Added
            && line.text.starts_with(b"Subproject commit ")
            && line.text.ends_with(b"-dirty")
        {
            new_dirty = true;
        }
    }
    SubmoduleChange {
        old_commit: file.old_oid.clone(),
        new_commit: file.new_oid.clone(),
        new_dirty,
    }
}

fn type_change_hunk(
    sections: &[FilePatch],
    opts: &super::models::AlignmentOptions,
) -> Result<Option<super::models::DiffHunk>> {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();
    for s in sections {
        for h in hunk::build_hunks(&s.hunks, opts)? {
            for row in h.rows {
                old_lines.extend(row.old_line);
                new_lines.extend(row.new_line);
            }
        }
    }
    if old_lines.is_empty() && new_lines.is_empty() {
        return Ok(None);
    }
    let range = |n: usize| match n {
        0 => "0,0".to_string(),
        1 => "1".to_string(),
        n => format!("1,{n}"),
    };
    let header = format!(
        "@@ -{} +{} @@",
        range(old_lines.len()),
        range(new_lines.len())
    )
    .into_bytes();
    let (old_count, new_count) = (old_lines.len(), new_lines.len());
    let (rows, alignment) = super::alignment::align(old_lines, new_lines, opts);
    Ok(Some(super::models::DiffHunk {
        old_start: usize::from(old_count > 0),
        old_count,
        new_start: usize::from(new_count > 0),
        new_count,
        header,
        section_heading: Vec::new(),
        rows,
        alignment,
    }))
}
