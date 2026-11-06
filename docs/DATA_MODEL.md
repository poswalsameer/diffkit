# Data Model

All types live in `src/git/models.rs`. They carry no formatting logic.

## RepositorySnapshot

```rust
pub struct RepositorySnapshot {
    pub repository: RepositoryInfo,
    pub branch: BranchInfo,
    pub upstream: UpstreamInfo,
    pub commit_tracking: CommitTrackingInfo,
    pub status: RepositoryStatus,
}
```

Produced by `git::snapshot::inspect(path)` (or `inspect_current_dir()`).

## RepositoryInfo

| Field | Type | Meaning |
|---|---|---|
| `root` | `PathBuf` | `git rev-parse --show-toplevel` |
| `git_dir` | `PathBuf` | This working tree's Git directory (`--absolute-git-dir`) |
| `common_git_dir` | `PathBuf` | Directory shared by all worktrees (`--git-common-dir`) |
| `current_directory` | `PathBuf` | Where inspection started (symlinks resolved on Unix) |
| `kind` | `RepositoryKind` | `Standard`, or `LinkedWorktree` when `git_dir != common_git_dir` |
| `is_inside_work_tree` | `bool` | Always `true` in a successful snapshot (bare repos and `.git` directories are errors) |
| `shallow` | `bool` | `--is-shallow-repository`; counts are limited to local history |
| `object_format` | `ObjectFormat` | `Sha1` or `Sha256` (`--show-object-format`); provides the empty-tree ID used as the base when HEAD is unborn |

## BranchInfo

```rust
pub enum HeadState { Branch(String), Detached, Unborn(String), Unknown }
pub struct BranchInfo {
    pub head: HeadState,
    pub head_commit: Option<ObjectId>,     // None when unborn
    pub empty_repository: Option<bool>,    // Some only when unborn
}
```

Helpers: `name()`, `detached()`, `unborn()`. A detached HEAD is never
represented as a branch named "HEAD". `empty_repository` separates a freshly
initialized repository (no refs) from an orphan branch in a repository that
has commits. `ObjectId` accepts only 40 or 64 lowercase hex digits.

## UpstreamInfo

```rust
pub enum UpstreamInfo {
    NotConfigured,                         // branch has no upstream
    NotApplicable,                         // HEAD is detached or unknown
    Missing { name: String },              // configured, tracking ref absent locally
    Available { name: String, full_ref: Option<String>, commit: ObjectId },
}
```

Helpers: `name()`, `commit()`, `configured()`, `available()`. `name` is
Git's short form (`origin/main`); `full_ref` is `refs/remotes/origin/main`
(or `refs/heads/main` for a local upstream). `full_ref` is `None` only for
an unborn branch, where Diffkit resolves just the commit. The commit is
whatever the *local* tracking ref points to; it may be stale.

## CommitTrackingInfo

```rust
pub enum CommitTrackingInfo {
    Compared { ahead: u64, behind: u64 },
    Unavailable(TrackingUnavailable),
}
pub enum TrackingUnavailable { NoUpstream, DetachedHead, UpstreamMissing, UnbornHead, NotReported }
pub enum TrackingRelation { UpToDate, Ahead, Behind, Diverged }
```

Helpers: `comparison_available()`, `ahead()`, `behind()` (both `None` when
unavailable, never a fabricated `0`), and `relation()`. Counts come from
Git's `# branch.ab` header, computed by commit-graph reachability exactly
like `git rev-list --left-right --count HEAD...@{upstream}`. Tests compare
every count with `rev-list`.

## FileStatus

| Field | Type | Meaning |
|---|---|---|
| `path` | `PathBuf` | Current path relative to the root, byte-exact |
| `original_path` | `Option<PathBuf>` | Source of a rename or copy |
| `index` | `Change` | X: index compared with HEAD |
| `worktree` | `Change` | Y: working tree compared with the index |
| `xy` | `[u8; 2]` | Raw code as Git printed it (`??` for untracked) |
| `conflict` | `Option<ConflictKind>` | `DD AU UD UA DU AA UU` |
| `similarity` | `Option<Similarity>` | Rename/copy score (`R87` → 87) |
| `file_kind` | `Option<FileKind>` | Regular / Executable / Symlink / Submodule / Other, from the worktree mode, falling back to index then HEAD; `None` for untracked |
| `submodule` | `Option<SubmoduleState>` | Parsed `S<c><m><u>` field |

`Change` is `Unmodified, Modified, TypeChanged, Added, Deleted, Renamed,
Copied, Unmerged, Untracked, Unknown(u8)`. An unrecognized letter is kept as
`Unknown(byte)` instead of being rejected or guessed.

### How states are represented

| Question | Predicate | Rule |
|---|---|---|
| Staged? | `is_staged()` | Not untracked, not conflicted, `index != Unmodified` |
| Unstaged? | `is_unstaged()` | Not untracked, not conflicted, `worktree != Unmodified` |
| Partially staged? | `is_partially_staged()` | Staged **and** unstaged (e.g. `MM`, `AM`, `RM`, `MD`) |
| Untracked? | `is_untracked()` | `index == Untracked` (`??` record) |
| Conflicted? | `is_conflicted()` | `conflict.is_some()` (`u` record); both sides are `Unmerged` |
| Deleted? | `is_deleted()` | Not conflicted, and either side is `Deleted` |
| Renamed / copied? | `is_renamed()` / `is_copied()` | Either side is `Renamed` / `Copied` |

The two dimensions are independent: neither is inferred from the other.
Intent-to-add (`.A`) is an unstaged addition. Conflicts are counted only as
conflicts, never as staged, unstaged, or deleted.

## RepositoryStatus

```rust
pub struct RepositoryStatus { pub files: Vec<FileStatus>, pub ignored: Vec<PathBuf> }
```

Counts: `staged_count()`, `unstaged_count()`, `partially_staged_count()`,
`untracked_count()`, `conflicted_count()`, `deleted_count()`,
`renamed_count()`, `copied_count()`, plus `is_clean()`. `ignored` stays
separate from `files`. Diffkit does not request ignored entries, so it is
empty in practice, but a `!` record would never be counted as untracked.

## Raw headers

`status::BranchHeaders` keeps `branch.oid`, `branch.head`,
`branch.upstream`, and `branch.ab` as raw bytes so the branch and commit
layers can interpret them. Unknown headers (e.g. `# stash`) are ignored.

# Phase 2 — Diff Models

All types live in `src/diff/models.rs` and are re-exported from
`diffkit::diff`.

## Comparison and identity

- `DiffComparison::{Staged, Unstaged, Combined, Untracked}`: HEAD → index,
  index → working tree, HEAD → working tree, untracked → working tree.
  `new_side_is_working_tree()` is true for all but `Staged`.
- `FileId(usize)`: index of a file within one `DiffFileList`. It is only
  valid for that list.

## Discovery: `DiffFileList` and `DiffFile`

```rust
pub struct DiffFileList { pub comparison, pub base: Option<ObjectId>, pub files: Vec<DiffFile>, pub stat_only_entries: usize }
pub struct DiffFile {
    pub id: FileId, pub comparison: DiffComparison,
    pub old_path: Option<PathBuf>,   // None for additions
    pub new_path: Option<PathBuf>,   // None for deletions
    pub kind: FileChangeKind,        // Added Modified Deleted Renamed Copied TypeChanged ModeChanged Unmerged Unknown(u8)
    pub old_mode: Option<FileMode>, pub new_mode: Option<FileMode>,
    pub similarity: Option<u8>,      // only for renames/copies, as Git reported
    pub old_oid: Option<ObjectId>, pub new_oid: Option<ObjectId>,  // None where Git printed the zero ID
    pub binary: BinaryStatus,        // Text | Binary | Unknown (untracked before loading)
    pub stats: Option<LineStats>,    // --numstat; None for binary, unmerged, untracked
    pub conflicted: bool,
    pub worktree_stamp: Option<WorktreeStamp>,  // lstat at discovery for working-tree sides
}
```

`path()` returns the new path, or the old path for deletions. `has_hunks()`
returns `Some(false)` for pure renames, mode changes, and empty files. List
helpers: `get(id)`, `len()`, `line_stats()`, `binary_count()`.
`stat_only_entries` counts working-tree files Git listed only because their
stat data changed.

## Loaded diff: `FileDiff`

```rust
pub struct FileDiff { pub file: DiffFile, pub content: DiffContent, pub source: ContentSource, pub old_oid, pub new_oid }
pub enum DiffContent {
    Text { hunks: Vec<DiffHunk> }, Binary(BinarySummary), Submodule(SubmoduleChange),
    Unmerged, TooLarge { limit, size: Option<u64> }, Unreadable { reason }, NestedRepository,
}
```

- `ContentSource` is `Git`, `WorkingTree` (untracked), or `Metadata`
  (unmerged entries, which need no Git call).
- `old_oid`/`new_oid` come from the patch's `index` line. Git hashes
  working-tree content in memory for that line; nothing is written.
- Helpers: `hunks()`, `rows()`, `line_stats()`.
- `BinarySummary { old_size, new_size }`: object sizes from
  `git cat-file -s`; a working-tree side uses the file size.
- `SubmoduleChange { old_commit, new_commit, new_dirty }`.

## Hunks, rows, and lines

```rust
pub struct DiffHunk { pub old_start, pub old_count, pub new_start, pub new_count,
                      pub header: Vec<u8>, pub section_heading: Vec<u8>,
                      pub rows: Vec<AlignedRow>, pub alignment: AlignmentQuality /* Exact | Bounded */ }
pub struct AlignedRow { pub kind: DiffRowKind, pub old_line: Option<DiffLine>, pub new_line: Option<DiffLine> }
pub enum DiffRowKind { Context, Added, Removed, Modified, Empty }
pub struct DiffLine { pub content: Vec<u8>, pub line_number: usize, pub ending: LineEnding /* Lf | CrLf | None */ }
```

| Row kind | `old_line` | `new_line` |
|---|---|---|
| `Context` | Some (same content) | Some |
| `Removed` | Some | None |
| `Added` | None | Some |
| `Modified` | Some | Some (a similar line, paired by the policy) |
| `Empty` | None | None (reserved for UI padding; never produced by the engine) |

Line numbers are 1-based and independent per side. Content is raw bytes
without the line ending. `ending` distinguishes LF, CRLF, and a missing
final newline (`has_newline()`).

## Options

`DiffOptions { context_lines: 3, max_file_patch_bytes: 64 MiB,
max_total_patch_bytes: 512 MiB, max_untracked_bytes: 8 MiB, alignment }`
and `AlignmentOptions { max_edit_cost: 4096, pairing_threshold: 40,
pairing_window: 16 }`.

## Errors

`DiffError::{Git(GitError), Parse { line, detail }, UnknownFile(FileId),
Stale { path, reason }, Vanished { path }, OutsideRepository { path },
Io { path, source }}`, with `StaleReason::{MissingFromDiff, ObjectChanged,
StatsChanged, BinaryChanged, WorkingTreeChanged}`.

Expected outcomes are `DiffContent` variants, not errors: binary, too large,
unreadable, unmerged, and nested repository.
