//! Typed repository state, independent of how it is displayed.
//!
//! Missing information is always explicit (`Option` or a dedicated enum
//! variant); it is never encoded as a misleading default such as a zero
//! ahead/behind count.

use std::{fmt, path::PathBuf};

/// A full hexadecimal Git object name (SHA-1 or SHA-256).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    /// Accepts 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let valid = matches!(bytes.len(), 40 | 64)
            && bytes
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
        // Only ASCII hex digits reach this point, so the conversion is exact.
        valid.then(|| Self(bytes.iter().map(|&b| b as char).collect()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The all-zero ID Git prints for "no object" or "not hashed".
    pub fn is_zero(&self) -> bool {
        self.0.bytes().all(|b| b == b'0')
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Hash algorithm of the repository's object database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    /// The empty tree, which Git knows without it being stored. Used as the
    /// base of comparisons when HEAD is unborn.
    pub fn empty_tree(self) -> ObjectId {
        let hex: &[u8] = match self {
            Self::Sha1 => b"4b825dc642cb6eb9a060e54bf8d69288fbee4904",
            Self::Sha256 => b"6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321",
        };
        ObjectId::parse(hex).expect("valid constant")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKind {
    /// The main working tree; its Git directory is the common directory.
    Standard,
    /// A worktree created with `git worktree add`; its Git directory lives
    /// under the common directory's `worktrees/`.
    LinkedWorktree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInfo {
    /// Top level of the working tree (`git rev-parse --show-toplevel`).
    pub root: PathBuf,
    /// The Git directory of this working tree (`--absolute-git-dir`).
    pub git_dir: PathBuf,
    /// The directory shared by all worktrees (`--git-common-dir`).
    pub common_git_dir: PathBuf,
    /// The canonicalized directory inspection started from.
    pub current_directory: PathBuf,
    pub kind: RepositoryKind,
    pub is_inside_work_tree: bool,
    /// History is truncated (`git clone --depth`); commit counts are limited
    /// to the locally available history.
    pub shallow: bool,
    pub object_format: ObjectFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadState {
    /// HEAD points at an existing branch.
    Branch(String),
    /// HEAD points directly at a commit.
    Detached,
    /// HEAD names a branch that has no commits yet.
    Unborn(String),
    /// Git did not report a usable branch name.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub head: HeadState,
    /// `None` when HEAD is unborn.
    pub head_commit: Option<ObjectId>,
    /// `Some` only for an unborn HEAD: whether the repository contains no
    /// references at all (a freshly initialized repository).
    pub empty_repository: Option<bool>,
}

impl BranchInfo {
    /// The branch name for a normal or unborn branch.
    pub fn name(&self) -> Option<&str> {
        match &self.head {
            HeadState::Branch(n) | HeadState::Unborn(n) => Some(n),
            HeadState::Detached | HeadState::Unknown => None,
        }
    }

    pub fn detached(&self) -> bool {
        self.head == HeadState::Detached
    }

    pub fn unborn(&self) -> bool {
        matches!(self.head, HeadState::Unborn(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamInfo {
    /// The current branch has no upstream configured.
    NotConfigured,
    /// HEAD is detached (or unknown), so there is no branch to track anything.
    NotApplicable,
    /// An upstream is configured but its tracking ref does not exist
    /// locally (never fetched, or deleted and pruned).
    Missing { name: String },
    /// The upstream ref exists locally.
    Available {
        /// Short name as Git reports it, e.g. `origin/main`.
        name: String,
        /// Full ref name, e.g. `refs/remotes/origin/main`, when resolved.
        full_ref: Option<String>,
        /// Commit the local tracking ref currently points at.
        commit: ObjectId,
    },
}

impl UpstreamInfo {
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Missing { name } | Self::Available { name, .. } => Some(name),
            Self::NotConfigured | Self::NotApplicable => None,
        }
    }

    pub fn commit(&self) -> Option<&ObjectId> {
        match self {
            Self::Available { commit, .. } => Some(commit),
            _ => None,
        }
    }

    pub fn configured(&self) -> bool {
        matches!(self, Self::Missing { .. } | Self::Available { .. })
    }

    pub fn available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Why ahead/behind counts could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingUnavailable {
    NoUpstream,
    DetachedHead,
    UpstreamMissing,
    UnbornHead,
    /// Git did not report counts (for example `+? -?`).
    NotReported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingRelation {
    UpToDate,
    Ahead,
    Behind,
    Diverged,
}

impl TrackingRelation {
    pub fn of(ahead: u64, behind: u64) -> Self {
        match (ahead, behind) {
            (0, 0) => Self::UpToDate,
            (_, 0) => Self::Ahead,
            (0, _) => Self::Behind,
            _ => Self::Diverged,
        }
    }
}

/// Comparison of HEAD with the *locally known* upstream ref. Counts come
/// from Git's commit graph and may be stale relative to the real remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitTrackingInfo {
    Compared { ahead: u64, behind: u64 },
    Unavailable(TrackingUnavailable),
}

impl CommitTrackingInfo {
    pub fn comparison_available(&self) -> bool {
        matches!(self, Self::Compared { .. })
    }

    pub fn ahead(&self) -> Option<u64> {
        match self {
            Self::Compared { ahead, .. } => Some(*ahead),
            Self::Unavailable(_) => None,
        }
    }

    pub fn behind(&self) -> Option<u64> {
        match self {
            Self::Compared { behind, .. } => Some(*behind),
            Self::Unavailable(_) => None,
        }
    }

    pub fn relation(&self) -> Option<TrackingRelation> {
        match *self {
            Self::Compared { ahead, behind } => Some(TrackingRelation::of(ahead, behind)),
            Self::Unavailable(_) => None,
        }
    }
}

/// One side (index or working tree) of a porcelain v2 `XY` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Unmodified,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
    /// Part of a merge conflict; see [`FileStatus::conflict`].
    Unmerged,
    Untracked,
    /// A status letter this version of Diffkit does not know.
    Unknown(u8),
}

/// The seven unmerged `XY` combinations Git reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// `DD`
    BothDeleted,
    /// `AU`
    AddedByUs,
    /// `UD`
    DeletedByThem,
    /// `UA`
    AddedByThem,
    /// `DU`
    DeletedByUs,
    /// `AA`
    BothAdded,
    /// `UU`
    BothModified,
}

impl ConflictKind {
    pub fn from_xy(xy: [u8; 2]) -> Option<Self> {
        Some(match &xy {
            b"DD" => Self::BothDeleted,
            b"AU" => Self::AddedByUs,
            b"UD" => Self::DeletedByThem,
            b"UA" => Self::AddedByThem,
            b"DU" => Self::DeletedByUs,
            b"AA" => Self::BothAdded,
            b"UU" => Self::BothModified,
            _ => return None,
        })
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::BothDeleted => "both deleted",
            Self::AddedByUs => "added by us",
            Self::DeletedByThem => "deleted by them",
            Self::AddedByThem => "added by them",
            Self::DeletedByUs => "deleted by us",
            Self::BothAdded => "both added",
            Self::BothModified => "both modified",
        }
    }
}

/// One index stage of an unmerged path: the file as it was in the merge
/// base (stage 1), on our side (stage 2), or on their side (stage 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictStage {
    /// Octal mode, for example `0o100644`.
    pub mode: u32,
    pub oid: ObjectId,
}

impl ConflictStage {
    pub fn kind(&self) -> Option<FileKind> {
        FileKind::from_mode(self.mode)
    }
}

/// The index stages Git recorded for an unmerged path. A stage is `None`
/// when that side has no version of the file (for example, `base` in an
/// add/add conflict, or `theirs` when they deleted it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictStages {
    pub base: Option<ConflictStage>,
    pub ours: Option<ConflictStage>,
    pub theirs: Option<ConflictStage>,
}

impl ConflictStages {
    /// The conflict kind implied by which stages exist, using the same
    /// mapping as Git's status (`DD` = base only, … `UU` = all three).
    pub fn kind(&self) -> Option<ConflictKind> {
        use ConflictKind::*;
        Some(
            match (
                self.base.is_some(),
                self.ours.is_some(),
                self.theirs.is_some(),
            ) {
                (true, false, false) => BothDeleted,
                (false, true, false) => AddedByUs,
                (true, true, false) => DeletedByThem,
                (false, false, true) => AddedByThem,
                (true, false, true) => DeletedByUs,
                (false, true, true) => BothAdded,
                (true, true, true) => BothModified,
                (false, false, false) => return None,
            },
        )
    }
}

/// Object type derived from a porcelain v2 octal mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Regular,
    Executable,
    Symlink,
    Submodule,
    Other(u32),
}

impl FileKind {
    pub fn from_mode(mode: u32) -> Option<Self> {
        Some(match mode {
            0 => return None,
            0o100644 => Self::Regular,
            0o100755 => Self::Executable,
            0o120000 => Self::Symlink,
            0o160000 => Self::Submodule,
            other => Self::Other(other),
        })
    }
}

/// The `<sub>` field: `N...` for non-submodules, `S<c><m><u>` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmoduleState {
    pub commit_changed: bool,
    pub has_tracked_changes: bool,
    pub has_untracked_changes: bool,
}

/// Similarity score of a rename (`R87`) or copy (`C100`) record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Similarity(pub u8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Current path, relative to the repository root, preserved byte-exact.
    pub path: PathBuf,
    /// Source path of a rename or copy.
    pub original_path: Option<PathBuf>,
    pub index: Change,
    pub worktree: Change,
    /// The raw `XY` code exactly as Git reported it (`??` for untracked).
    pub xy: [u8; 2],
    pub conflict: Option<ConflictKind>,
    pub similarity: Option<Similarity>,
    /// From the worktree mode, falling back to the index and HEAD modes;
    /// `None` for untracked files (porcelain does not report a mode).
    pub file_kind: Option<FileKind>,
    pub submodule: Option<SubmoduleState>,
    /// Index stages of an unmerged path (`u` records only).
    pub stages: Option<ConflictStages>,
}

impl FileStatus {
    pub fn is_untracked(&self) -> bool {
        self.index == Change::Untracked
    }

    pub fn is_conflicted(&self) -> bool {
        self.conflict.is_some()
    }

    /// Has changes recorded in the index relative to HEAD.
    pub fn is_staged(&self) -> bool {
        !self.is_untracked() && !self.is_conflicted() && self.index != Change::Unmodified
    }

    /// Has working-tree changes relative to the index.
    pub fn is_unstaged(&self) -> bool {
        !self.is_untracked() && !self.is_conflicted() && self.worktree != Change::Unmodified
    }

    pub fn is_partially_staged(&self) -> bool {
        self.is_staged() && self.is_unstaged()
    }

    pub fn is_deleted(&self) -> bool {
        !self.is_conflicted() && (self.index == Change::Deleted || self.worktree == Change::Deleted)
    }

    pub fn is_renamed(&self) -> bool {
        self.index == Change::Renamed || self.worktree == Change::Renamed
    }

    pub fn is_copied(&self) -> bool {
        self.index == Change::Copied || self.worktree == Change::Copied
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepositoryStatus {
    /// Tracked changes, conflicts, and untracked files in Git's order.
    pub files: Vec<FileStatus>,
    /// Ignored paths, only populated if Git was asked to report them. They
    /// are never mixed into `files`.
    pub ignored: Vec<PathBuf>,
}

impl RepositoryStatus {
    fn count(&self, f: impl Fn(&FileStatus) -> bool) -> usize {
        self.files.iter().filter(|s| f(s)).count()
    }

    pub fn staged_count(&self) -> usize {
        self.count(FileStatus::is_staged)
    }

    pub fn unstaged_count(&self) -> usize {
        self.count(FileStatus::is_unstaged)
    }

    pub fn partially_staged_count(&self) -> usize {
        self.count(FileStatus::is_partially_staged)
    }

    pub fn untracked_count(&self) -> usize {
        self.count(FileStatus::is_untracked)
    }

    pub fn conflicted_count(&self) -> usize {
        self.count(FileStatus::is_conflicted)
    }

    pub fn deleted_count(&self) -> usize {
        self.count(FileStatus::is_deleted)
    }

    pub fn renamed_count(&self) -> usize {
        self.count(FileStatus::is_renamed)
    }

    pub fn copied_count(&self) -> usize {
        self.count(FileStatus::is_copied)
    }

    /// No staged, unstaged, untracked, or conflicted entries.
    pub fn is_clean(&self) -> bool {
        self.files.is_empty()
    }
}

/// How a rebase in progress is being run.
///
/// Since Git 2.26, `git rebase` and `git rebase -i` both use the sequencer
/// ("merge") backend and leave identical state files (Git's own status
/// calls both "interactive rebase in progress"), so they are not told
/// apart here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseBackend {
    /// The sequencer backend (`rebase-merge/`): `git rebase`, `git rebase -i`.
    Merge,
    /// The patch-based backend (`rebase-apply/`): `git rebase --apply`.
    Apply,
}

/// A multi-step Git operation that has started and not yet finished in
/// this worktree. Detected from the state files Git keeps in the
/// per-worktree Git directory, the same files `git status` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// `MERGE_HEAD` exists.
    Merge,
    Rebase {
        backend: RebaseBackend,
        /// (current step, total steps), when Git recorded them.
        step: Option<(u64, u64)>,
        /// The branch being rebased, as Git recorded it (for example
        /// `refs/heads/feature`), or `None` for a detached rebase.
        branch: Option<String>,
        /// The commit being rebased onto.
        onto: Option<ObjectId>,
    },
    /// `git am` is applying patches from a mailbox.
    ApplyMailbox { step: Option<(u64, u64)> },
    /// `CHERRY_PICK_HEAD` exists, or a cherry-pick sequence is in progress.
    CherryPick,
    /// `REVERT_HEAD` exists, or a revert sequence is in progress.
    Revert,
    /// `git bisect` has been started.
    Bisect {
        /// What bisect will return to (`BISECT_START`), usually a branch.
        start: Option<String>,
    },
}

/// Everything Phase 1 knows about a repository, collected in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySnapshot {
    pub repository: RepositoryInfo,
    pub branch: BranchInfo,
    pub upstream: UpstreamInfo,
    pub commit_tracking: CommitTrackingInfo,
    pub status: RepositoryStatus,
    /// Operations in progress (merge, rebase, cherry-pick, revert, bisect),
    /// in that order. Empty when none is.
    pub operations: Vec<Operation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(xy: &[u8; 2]) -> FileStatus {
        let side = |b| match b {
            b'.' => Change::Unmodified,
            b'M' => Change::Modified,
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'T' => Change::TypeChanged,
            other => Change::Unknown(other),
        };
        FileStatus {
            path: PathBuf::from("f"),
            original_path: None,
            index: side(xy[0]),
            worktree: side(xy[1]),
            xy: *xy,
            conflict: None,
            similarity: None,
            file_kind: Some(FileKind::Regular),
            submodule: None,
            stages: None,
        }
    }

    #[test]
    fn staged_and_unstaged_are_independent_dimensions() {
        let cases: &[(&[u8; 2], bool, bool)] = &[
            (b"M.", true, false),
            (b".M", false, true),
            (b"MM", true, true),
            (b"A.", true, false),
            (b"AM", true, true),
            (b"AD", true, true),
            (b"D.", true, false),
            (b".D", false, true),
            (b"R.", true, false),
            (b"RM", true, true),
            (b".R", false, true),
            (b"C.", true, false),
            (b"T.", true, false),
            (b".T", false, true),
            (b".A", false, true),
        ];
        for (xy, staged, unstaged) in cases {
            let e = entry(xy);
            let label = String::from_utf8_lossy(&xy[..]).into_owned();
            assert_eq!(e.is_staged(), *staged, "{label} staged");
            assert_eq!(e.is_unstaged(), *unstaged, "{label} unstaged");
            assert_eq!(e.is_partially_staged(), *staged && *unstaged, "{label}");
        }
    }

    #[test]
    fn conflicts_and_untracked_are_neither_staged_nor_unstaged() {
        let mut c = entry(b"UU");
        c.index = Change::Unmerged;
        c.worktree = Change::Unmerged;
        c.conflict = ConflictKind::from_xy(*b"UU");
        assert!(c.is_conflicted() && !c.is_staged() && !c.is_unstaged());
        let mut u = entry(b"??");
        u.index = Change::Untracked;
        u.worktree = Change::Untracked;
        assert!(u.is_untracked() && !u.is_staged() && !u.is_unstaged());
    }

    #[test]
    fn status_counts() {
        let s = RepositoryStatus {
            files: vec![entry(b"MM"), entry(b".D"), entry(b"R."), entry(b"C.")],
            ignored: vec![],
        };
        assert_eq!(s.staged_count(), 3);
        assert_eq!(s.unstaged_count(), 2);
        assert_eq!(s.partially_staged_count(), 1);
        assert_eq!(s.deleted_count(), 1);
        assert_eq!(s.renamed_count(), 1);
        assert_eq!(s.copied_count(), 1);
        assert!(!s.is_clean());
        assert!(RepositoryStatus::default().is_clean());
    }

    #[test]
    fn tracking_relation_and_unavailable_counts() {
        use TrackingRelation::*;
        let c = |ahead, behind| CommitTrackingInfo::Compared { ahead, behind };
        assert_eq!(c(0, 0).relation(), Some(UpToDate));
        assert_eq!(c(3, 0).relation(), Some(Ahead));
        assert_eq!(c(0, 2).relation(), Some(Behind));
        assert_eq!(c(2, 3).relation(), Some(Diverged));
        let none = CommitTrackingInfo::Unavailable(TrackingUnavailable::NoUpstream);
        assert_eq!(
            (none.ahead(), none.behind(), none.relation()),
            (None, None, None)
        );
        assert!(!none.comparison_available());
    }

    #[test]
    fn object_id_validation() {
        assert!(ObjectId::parse(&[b'a'; 40]).is_some());
        assert!(ObjectId::parse(&[b'0'; 64]).is_some());
        assert!(ObjectId::parse(&[b'a'; 39]).is_none());
        assert!(ObjectId::parse(&[b'G'; 40]).is_none());
        assert!(ObjectId::parse(b"(initial)").is_none());
    }

    #[test]
    fn conflict_codes_and_modes() {
        for xy in [b"DD", b"AU", b"UD", b"UA", b"DU", b"AA", b"UU"] {
            assert!(ConflictKind::from_xy(*xy).is_some());
        }
        assert_eq!(ConflictKind::from_xy(*b"MM"), None);
        assert_eq!(FileKind::from_mode(0o120000), Some(FileKind::Symlink));
        assert_eq!(FileKind::from_mode(0), None);
    }

    #[test]
    fn conflict_kind_follows_the_stages_present() {
        let stage = || {
            Some(ConflictStage {
                mode: 0o100644,
                oid: ObjectId::parse(&[b'a'; 40]).unwrap(),
            })
        };
        let s = |b: bool, o: bool, t: bool| ConflictStages {
            base: b.then(stage).flatten(),
            ours: o.then(stage).flatten(),
            theirs: t.then(stage).flatten(),
        };
        let cases = [
            (s(true, false, false), b"DD"),
            (s(false, true, false), b"AU"),
            (s(true, true, false), b"UD"),
            (s(false, false, true), b"UA"),
            (s(true, false, true), b"DU"),
            (s(false, true, true), b"AA"),
            (s(true, true, true), b"UU"),
        ];
        for (stages, xy) in cases {
            assert_eq!(stages.kind(), ConflictKind::from_xy(*xy), "{xy:?}");
        }
        assert_eq!(s(false, false, false).kind(), None);
    }
}
