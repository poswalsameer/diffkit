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
//!
//! A commit is inspected the same way:
//! [`DiffEngine::discover_commit`] lists its files against its first parent,
//! and [`DiffEngine::load_file_diff`] loads one of them. Commits never
//! change, so those lists cannot go stale.

use super::{
    binary, commands, discovery,
    errors::{DiffError, Result, StaleReason},
    hunk,
    models::{
        BinaryStatus, ConflictInfo, ContentSource, DiffCollection, DiffComparison, DiffContent,
        DiffFile, DiffFileList, DiffOptions, FileChangeKind, FileDiff, FileId, LineStats,
        LoadedFile, SubmoduleChange, SubmoduleWorktree,
    },
    parser::{self, FilePatch, Origin, PatchKind},
    untracked,
};
use crate::git::{
    self,
    commands::{path_bytes, LimitedOutput},
    models::{
        ConflictKind, ConflictStages, ObjectFormat, ObjectId, RepositorySnapshot, RepositoryStatus,
    },
    snapshot,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Most path bytes [`DiffEngine::discover_reported`] passes on one command
/// line; above it the whole tree is compared. Far below the smallest
/// platform limit (32,767 UTF-16 units for a Windows command line).
pub const MAX_PATHSPEC_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct DiffEngine {
    root: PathBuf,
    head: Option<ObjectId>,
    object_format: ObjectFormat,
    /// Kind and index stages of each conflicted path, from the snapshot.
    conflicts: HashMap<PathBuf, (Option<ConflictKind>, Option<ConflictStages>)>,
    options: DiffOptions,
}

/// (old path, new path) as Git bytes; identifies a file section in a patch.
type PathKey = (Option<Vec<u8>>, Option<Vec<u8>>);

impl DiffEngine {
    /// Builds an engine from an existing Phase 1 snapshot. HEAD and the
    /// conflicted paths are taken from it; no Git command runs.
    pub fn from_snapshot(s: &RepositorySnapshot, options: DiffOptions) -> Self {
        Self {
            root: s.repository.root.clone(),
            head: s.branch.head_commit.clone(),
            object_format: s.repository.object_format,
            conflicts: s
                .status
                .files
                .iter()
                .filter(|f| f.is_conflicted())
                .map(|f| (f.path.clone(), (f.conflict, f.stages.clone())))
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
    /// [`DiffComparison::Commit`] needs a commit; use
    /// [`DiffEngine::discover_commit`].
    pub fn discover(&self, comparison: DiffComparison) -> Result<DiffFileList> {
        match comparison {
            DiffComparison::Untracked => Ok(DiffFileList {
                comparison,
                base: None,
                target: None,
                files: untracked::discover(&self.root)?,
                stat_only_entries: 0,
            }),
            DiffComparison::Commit => Err(DiffError::parse(
                0,
                "commit inspection needs a commit (use discover_commit)",
            )),
            _ => self.discover_tracked(comparison, None),
        }
    }

    /// Like [`DiffEngine::discover`], but asks Git only about the paths that
    /// `status` (taken just before, from the same repository) reports:
    ///
    /// - tracked comparisons compare only the tracked paths status lists
    ///   (both sides of a rename or copy), and run no Git command when
    ///   there are none. Every comparison gets all of them, not just those
    ///   status files under that side: the plumbing commands can classify a
    ///   path differently (an intent-to-add file is a working-tree change
    ///   for status but an addition for `diff-index --cached`);
    /// - the untracked list is status's `?` records, without another walk.
    ///
    /// The whole-tree commands also re-examine every file whose stat data
    /// changed but whose content did not (Diffkit never refreshes the
    /// index), so each would repeat the content checks status just made.
    /// Above [`MAX_PATHSPEC_BYTES`] of paths, the whole tree is compared, to
    /// stay within command-line limits. Files whose stat data alone changed
    /// are not listed either way; [`DiffFileList::stat_only_entries`] only
    /// counts the ones a whole-tree comparison examined.
    pub fn discover_reported(
        &self,
        comparison: DiffComparison,
        status: &RepositoryStatus,
    ) -> Result<DiffFileList> {
        use DiffComparison::*;
        match comparison {
            Commit => self.discover(comparison),
            Untracked => Ok(DiffFileList {
                comparison,
                base: None,
                target: None,
                files: untracked::from_git_paths(
                    &self.root,
                    status
                        .files
                        .iter()
                        .filter(|f| f.is_untracked())
                        .map(|f| path_bytes(&f.path)),
                )?,
                stat_only_entries: 0,
            }),
            Staged | Unstaged | Combined => {
                let mut paths: Vec<&Path> = Vec::new();
                for f in status.files.iter().filter(|f| !f.is_untracked()) {
                    paths.push(&f.path);
                    paths.extend(f.original_path.as_deref());
                }
                let size: usize = paths.iter().map(|p| p.as_os_str().len() + 1).sum();
                if size > MAX_PATHSPEC_BYTES {
                    self.discover_tracked(comparison, None)
                } else {
                    self.discover_tracked(comparison, Some(&paths))
                }
            }
        }
    }

    /// Staged, unstaged, or combined discovery: of the whole tree, or of
    /// `paths` only (none: an empty list, without running Git).
    fn discover_tracked(
        &self,
        comparison: DiffComparison,
        paths: Option<&[&Path]>,
    ) -> Result<DiffFileList> {
        let base = self.base();
        let range = commands::Range {
            base: &base,
            target: None,
        };
        let found = match paths {
            Some([]) => discovery::Discovered {
                pairs: Vec::new(),
                stat_only: 0,
            },
            Some(paths) => discovery::parse_raw_numstat(&git::commands::run(
                &self.root,
                &commands::discovery_of(comparison, range, paths),
            )?)?,
            None => discovery::parse_raw_numstat(&git::commands::run(
                &self.root,
                &commands::discovery(comparison, range),
            )?)?,
        };
        let files = discovery::build_files(found.pairs, comparison, &self.root, |p| {
            self.conflicts.contains_key(p)
        })?;
        Ok(DiffFileList {
            comparison,
            base: (comparison != DiffComparison::Unstaged).then_some(base),
            target: None,
            files,
            stat_only_entries: found.stat_only,
        })
    }

    /// Lists the files changed by `commit` against `first_parent` (the empty
    /// tree when `None`, for a root commit). Merge commits are compared with
    /// their first parent only. Metadata only; no content is loaded.
    pub fn discover_commit(
        &self,
        commit: &ObjectId,
        first_parent: Option<&ObjectId>,
    ) -> Result<DiffFileList> {
        let base = first_parent
            .cloned()
            .unwrap_or_else(|| self.object_format.empty_tree());
        let range = commands::Range {
            base: &base,
            target: Some(commit),
        };
        let args = commands::discovery(DiffComparison::Commit, range);
        let out = git::commands::run(&self.root, &args)?;
        let found = discovery::parse_raw_numstat(&out)?;
        let files =
            discovery::build_files(found.pairs, DiffComparison::Commit, &self.root, |_| false)?;
        Ok(DiffFileList {
            comparison: DiffComparison::Commit,
            base: Some(base),
            target: Some(commit.clone()),
            files,
            stat_only_entries: found.stat_only,
        })
    }

    /// The trees a list compares. Working comparisons use the engine's
    /// HEAD; a commit list carries its own parent and commit.
    fn range_of(list: &DiffFileList, head: &ObjectId) -> (ObjectId, Option<ObjectId>) {
        match list.comparison {
            DiffComparison::Commit => (
                list.base.clone().unwrap_or_else(|| head.clone()),
                list.target.clone(),
            ),
            _ => (head.clone(), None),
        }
    }

    /// Loads the diff of one discovered file.
    pub fn load_file_diff(&self, list: &DiffFileList, id: FileId) -> Result<FileDiff> {
        let file = list.get(id).ok_or(DiffError::UnknownFile(id))?;
        if file.comparison == DiffComparison::Untracked {
            return untracked::load(&self.root, file, self.options.max_untracked_bytes);
        }
        if file.kind == FileChangeKind::Unmerged {
            return Ok(self.unmerged(file));
        }
        self.check_worktree(file)?;
        let mut paths: Vec<&Path> = file
            .old_path
            .iter()
            .chain(&file.new_path)
            .map(PathBuf::as_path)
            .collect();
        paths.dedup();
        let (base, target) = Self::range_of(list, &self.base());
        let range = commands::Range {
            base: &base,
            target: target.as_ref(),
        };
        let args = commands::patch(file.comparison, range, self.options.context_lines, &paths);
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
        let mut files = Vec::with_capacity(list.len());
        let unexpected_paths = self.load_each(list, |id, result| {
            files.push(LoadedFile { id, result });
        })?;
        Ok(DiffCollection {
            comparison: list.comparison,
            files,
            unexpected_paths,
        })
    }

    /// Like [`DiffEngine::load_all`], but hands each file's result to
    /// `each` as soon as it is built, exactly once per file and in list
    /// order, instead of keeping them all. The patch text of a file is freed
    /// once its result is built, so a caller that summarizes and drops each
    /// result holds at most the patch and one built file at a time. Returns
    /// the paths Git reported that discovery did not see.
    pub fn load_each(
        &self,
        list: &DiffFileList,
        mut each: impl FnMut(FileId, Result<FileDiff>),
    ) -> Result<Vec<PathBuf>> {
        let one_by_one = |each: &mut dyn FnMut(FileId, Result<FileDiff>)| {
            for f in &list.files {
                each(f.id, self.load_file_diff(list, f.id));
            }
            Ok(Vec::new())
        };
        if list.comparison == DiffComparison::Untracked || list.is_empty() {
            return one_by_one(&mut each);
        }
        let (base, target) = Self::range_of(list, &self.base());
        let range = commands::Range {
            base: &base,
            target: target.as_ref(),
        };
        let args = commands::patch(list.comparison, range, self.options.context_lines, &[]);
        let out = match git::commands::output_limited(
            &self.root,
            &args,
            self.options.max_total_patch_bytes,
        )? {
            LimitedOutput::Complete(out) if out.status.success() => out,
            LimitedOutput::Complete(out) => return Err(git::commands::failure(&args, &out).into()),
            LimitedOutput::Exceeded { .. } => return one_by_one(&mut each),
        };
        let parsed = parser::parse_patch(&out.stdout)?;
        drop(out);
        let mut sections: Vec<Option<FilePatch>> = parsed.into_iter().map(Some).collect();
        let mut by_key: HashMap<PathKey, std::collections::VecDeque<usize>> = HashMap::new();
        for (i, s) in sections.iter().enumerate() {
            by_key
                .entry(key_of(s.as_ref().expect("fresh")))
                .or_default()
                .push_back(i);
        }
        for file in &list.files {
            let mut matched = Vec::new();
            let mut missing = false;
            for key in section_keys(file) {
                match by_key.get_mut(&key).and_then(|v| v.pop_front()) {
                    Some(i) => matched.push(sections[i].take().expect("each section used once")),
                    None => missing = true,
                }
            }
            let result = if file.kind == FileChangeKind::Unmerged {
                Ok(self.unmerged(file))
            } else if missing {
                Err(stale(file, StaleReason::MissingFromDiff))
            } else {
                self.check_worktree(file)
                    .and_then(|()| self.build(file, matched))
            };
            each(file.id, result);
        }
        let mut unexpected_paths = Vec::new();
        for s in sections.into_iter().flatten() {
            if let Some(p) = s.new_path.or(s.old_path) {
                unexpected_paths.push(git::commands::path_from_git(&p)?);
            }
        }
        Ok(unexpected_paths)
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
            let mut change = submodule_change(file, &sections);
            if file.comparison != DiffComparison::Commit {
                change.worktree = Some(submodule_worktree(&self.root.join(file.path())));
            }
            DiffContent::Submodule(change)
        } else if is_binary {
            DiffContent::Binary(binary::summary(
                &self.root,
                file,
                file.old_oid.as_ref().or(old_oid.as_ref()),
                file.new_oid.as_ref().or(new_oid.as_ref()),
            )?)
        } else if sections.len() == 1 {
            let only = sections.into_iter().next().expect("one section");
            let hunks = hunk::build_hunks(only.hunks, &self.options.alignment)?;
            DiffContent::Text { hunks }
        } else {
            // A type change is printed as a deletion and an addition; present
            // it as one hunk from the old content to the new content.
            DiffContent::Text {
                hunks: type_change_hunk(sections, &self.options.alignment)?
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

impl DiffEngine {
    /// An unmerged path: the stages Git reported in the snapshot, and
    /// whether the working-tree file exists. No Git process runs.
    fn unmerged(&self, file: &DiffFile) -> FileDiff {
        let path = file.path();
        let (kind, stages) = self.conflicts.get(path).cloned().unwrap_or((None, None));
        let info = ConflictInfo {
            kind: kind.or_else(|| stages.as_ref().and_then(ConflictStages::kind)),
            stages,
            worktree_present: std::fs::symlink_metadata(self.root.join(path)).is_ok(),
        };
        FileDiff {
            file: file.clone(),
            content: DiffContent::Unmerged(info),
            source: ContentSource::Metadata,
            old_oid: None,
            new_oid: None,
        }
    }
}

/// What exists at a submodule's path, without looking inside it beyond
/// checking for `.git`.
fn submodule_worktree(path: &Path) -> SubmoduleWorktree {
    match std::fs::symlink_metadata(path) {
        Err(_) => SubmoduleWorktree::Missing,
        Ok(m) if !m.is_dir() => SubmoduleWorktree::NotADirectory,
        Ok(_) if std::fs::symlink_metadata(path.join(".git")).is_ok() => {
            SubmoduleWorktree::Populated
        }
        Ok(_) => SubmoduleWorktree::NotInitialized,
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
        worktree: None,
    }
}

fn type_change_hunk(
    sections: Vec<FilePatch>,
    opts: &super::models::AlignmentOptions,
) -> Result<Option<super::models::DiffHunk>> {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();
    for s in sections {
        for h in hunk::build_hunks(s.hunks, opts)? {
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
