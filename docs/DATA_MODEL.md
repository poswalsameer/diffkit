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
