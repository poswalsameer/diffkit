//! Structured diff data: discovered file metadata, loaded hunks, and rows
//! ready for side-by-side presentation. No type here does I/O or display.

use crate::git::models::{FileKind, ObjectId};
use std::{path::Path, path::PathBuf, time::SystemTime};

/// Which two states of the repository are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffComparison {
    /// `git diff --cached`: HEAD → index.
    Staged,
    /// `git diff`: index → working tree.
    Unstaged,
    /// `git diff HEAD`: HEAD → working tree, for tracked files.
    Combined,
    /// Files Git does not track and does not ignore, shown as additions.
    Untracked,
}

impl DiffComparison {
    pub const ALL: [Self; 4] = [
        Self::Staged,
        Self::Unstaged,
        Self::Combined,
        Self::Untracked,
    ];

    pub fn description(self) -> &'static str {
        match self {
            Self::Staged => "HEAD -> index",
            Self::Unstaged => "index -> working tree",
            Self::Combined => "HEAD -> working tree",
            Self::Untracked => "untracked -> working tree",
        }
    }

    /// The new side is read from the working tree (and can change at any
    /// moment), rather than from an object in the repository.
    pub fn new_side_is_working_tree(self) -> bool {
        !matches!(self, Self::Staged)
    }
}

/// Identity of a file within one [`DiffFileList`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    /// Only the mode (for example the executable bit) changed.
    ModeChanged,
    Unmerged,
    /// A status letter Diffkit does not know; the raw letter is kept.
    Unknown(u8),
}

/// A Git file mode such as `100644`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMode(pub u32);

impl FileMode {
    pub fn kind(self) -> Option<FileKind> {
        FileKind::from_mode(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryStatus {
    Text,
    Binary,
    /// Not determined without reading content (untracked files before
    /// loading).
    Unknown,
}

/// Line counts as Git reports them (`--numstat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineStats {
    pub added: u64,
    pub removed: u64,
}

/// File-system metadata of a working-tree file captured at discovery, used
/// to detect that the file changed before its diff was loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStamp {
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) file_type: StampType,
    #[cfg(unix)]
    pub(crate) inode: u64,
    #[cfg(unix)]
    pub(crate) mode: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StampType {
    File,
    Symlink,
    Directory,
    Other,
}

impl WorktreeStamp {
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Metadata of one changed file, produced by discovery without loading its
/// content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub id: FileId,
    pub comparison: DiffComparison,
    /// `None` for additions.
    pub old_path: Option<PathBuf>,
    /// `None` for deletions.
    pub new_path: Option<PathBuf>,
    pub kind: FileChangeKind,
    pub old_mode: Option<FileMode>,
    pub new_mode: Option<FileMode>,
    /// Rename/copy similarity reported by Git.
    pub similarity: Option<u8>,
    /// Object IDs as Git reported them; `None` when Git printed the zero ID
    /// (no object, or a working-tree file it did not hash).
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub binary: BinaryStatus,
    /// `None` for binary files, unmerged entries, and untracked files.
    pub stats: Option<LineStats>,
    /// The path is in a merge conflict (from `U` records or, for the
    /// combined comparison, from the repository status).
    pub conflicted: bool,
    /// Working-tree metadata at discovery, for working-tree sides.
    pub worktree_stamp: Option<WorktreeStamp>,
}

impl DiffFile {
    /// The new path, or the old path for deletions.
    pub fn path(&self) -> &Path {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .expect("a diff entry always has at least one path")
    }

    /// Whether loading will produce text hunks, when knowable from Git's
    /// metadata. `None` for binary, unmerged, or untracked entries.
    pub fn has_hunks(&self) -> Option<bool> {
        self.stats.map(|s| s.added + s.removed > 0)
    }
}

/// The result of discovery for one comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileList {
    pub comparison: DiffComparison,
    /// The commit (or, for an unborn HEAD, the empty tree) used as HEAD.
    /// `None` for comparisons that do not involve HEAD.
    pub base: Option<ObjectId>,
    pub files: Vec<DiffFile>,
    /// Working-tree files Git listed because their file-system metadata
    /// changed, but whose content is unchanged; they are not in `files`.
    pub stat_only_entries: usize,
}

impl DiffFileList {
    pub fn get(&self, id: FileId) -> Option<&DiffFile> {
        self.files.get(id.0).filter(|f| f.id == id)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Sum of Git's line counts over text files.
    pub fn line_stats(&self) -> LineStats {
        self.files
            .iter()
            .filter_map(|f| f.stats)
            .fold(LineStats::default(), |a, s| LineStats {
                added: a.added + s.added,
                removed: a.removed + s.removed,
            })
    }

    pub fn binary_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.binary == BinaryStatus::Binary)
            .count()
    }
}

/// How a line ended in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineEnding {
    Lf,
    CrLf,
    /// Last line of a file without a final newline.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffLine {
    /// Line content without its line ending. Not assumed to be UTF-8.
    pub content: Vec<u8>,
    /// 1-based line number in the side the line belongs to.
    pub line_number: usize,
    pub ending: LineEnding,
}

impl DiffLine {
    pub fn has_newline(&self) -> bool {
        self.ending != LineEnding::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowKind {
    /// Same line on both sides.
    Context,
    /// Only on the new side.
    Added,
    /// Only on the old side.
    Removed,
    /// An old line and a similar new line paired for side-by-side display.
    Modified,
    /// Neither side. Reserved for presentation-level padding; the engine
    /// never produces it.
    Empty,
}

/// One row of a side-by-side view. A missing side is `None`, never an empty
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedRow {
    pub kind: DiffRowKind,
    pub old_line: Option<DiffLine>,
    pub new_line: Option<DiffLine>,
}

/// Whether the Myers alignment of every change block ran to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignmentQuality {
    /// A shortest edit script was found for every block.
    Exact,
    /// A block exceeded [`AlignmentOptions::max_edit_cost`]; part of it was
    /// aligned as delete-then-insert without searching for common lines.
    Bounded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    /// The full `@@ … @@` line as Git printed it (or as synthesized for
    /// untracked files).
    pub header: Vec<u8>,
    /// Text after the closing `@@` (Git's function context), if any.
    pub section_heading: Vec<u8>,
    pub rows: Vec<AlignedRow>,
    pub alignment: AlignmentQuality,
}

/// Sizes of a binary change. Object sizes come from `git cat-file -s`;
/// a working-tree side reports the file size on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinarySummary {
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmoduleChange {
    pub old_commit: Option<ObjectId>,
    pub new_commit: Option<ObjectId>,
    /// Git reported the submodule's working tree as dirty (`-dirty`).
    pub new_dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffContent {
    /// Text change; `hunks` is empty for pure renames, copies, mode changes,
    /// and empty files.
    Text {
        hunks: Vec<DiffHunk>,
    },
    Binary(BinarySummary),
    Submodule(SubmoduleChange),
    /// A merge conflict. Git provides no two-way diff for this comparison.
    Unmerged,
    /// Content exceeded the configured limit; nothing partial is returned.
    TooLarge {
        limit: u64,
        size: Option<u64>,
    },
    /// An untracked file that could not be read.
    Unreadable {
        reason: String,
    },
    /// An untracked directory that is itself a Git repository.
    NestedRepository,
}

/// Where the content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSource {
    /// Parsed from `git diff` output.
    Git,
    /// Read directly from the working tree (untracked files).
    WorkingTree,
    /// Determined from discovery metadata without running Git again.
    Metadata,
}

/// A loaded diff for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub file: DiffFile,
    pub content: DiffContent,
    pub source: ContentSource,
    /// Object IDs from the patch's `index` line (Git hashes working-tree
    /// content in memory for this line; nothing is written).
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
}

impl FileDiff {
    pub fn hunks(&self) -> &[DiffHunk] {
        match &self.content {
            DiffContent::Text { hunks } => hunks,
            _ => &[],
        }
    }

    pub fn rows(&self) -> impl Iterator<Item = &AlignedRow> {
        self.hunks().iter().flat_map(|h| h.rows.iter())
    }

    /// Lines removed and added, counted from the hunks.
    pub fn line_stats(&self) -> LineStats {
        let mut s = LineStats::default();
        for row in self.rows() {
            match row.kind {
                DiffRowKind::Added => s.added += 1,
                DiffRowKind::Removed => s.removed += 1,
                DiffRowKind::Modified => {
                    s.added += 1;
                    s.removed += 1;
                }
                DiffRowKind::Context | DiffRowKind::Empty => {}
            }
        }
        s
    }
}

/// Result of loading one file as part of [`DiffCollection`].
#[derive(Debug)]
pub struct LoadedFile {
    pub id: FileId,
    pub result: Result<FileDiff, super::errors::DiffError>,
}

/// Every file of a [`DiffFileList`], loaded with one Git invocation.
#[derive(Debug)]
pub struct DiffCollection {
    pub comparison: DiffComparison,
    pub files: Vec<LoadedFile>,
    /// Paths Git reported at load time that were not in the discovered list
    /// (the repository changed after discovery).
    pub unexpected_paths: Vec<PathBuf>,
}

/// Limits for Myers alignment and line pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignmentOptions {
    /// Maximum edit distance searched per change block. Beyond it, the
    /// remaining part of the block is aligned as delete-then-insert.
    pub max_edit_cost: usize,
    /// Minimum similarity (percent) for a removed and an added line to be
    /// shown as one `Modified` row.
    pub pairing_threshold: u8,
    /// How many added lines ahead are considered when pairing a removed line.
    pub pairing_window: usize,
}

impl Default for AlignmentOptions {
    fn default() -> Self {
        Self {
            max_edit_cost: 4096,
            pairing_threshold: 40,
            pairing_window: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOptions {
    /// Unified context lines (`-U`).
    pub context_lines: u32,
    /// Maximum `git diff` output for one file.
    pub max_file_patch_bytes: u64,
    /// Maximum `git diff` output for [`load_all`](super::DiffEngine::load_all).
    pub max_total_patch_bytes: u64,
    /// Untracked files larger than this are reported as `TooLarge` unread.
    pub max_untracked_bytes: u64,
    pub alignment: AlignmentOptions,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context_lines: 3,
            max_file_patch_bytes: 64 << 20,
            max_total_patch_bytes: 512 << 20,
            max_untracked_bytes: 8 << 20,
            alignment: AlignmentOptions::default(),
        }
    }
}
