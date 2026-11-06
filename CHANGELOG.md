# Changelog

All notable changes to Diffkit. Versions follow semantic versioning.

## 1.0.0 — unreleased

Not tagged or published. Release readiness is tracked in
[docs/RELEASE_CHECKLIST.md](docs/RELEASE_CHECKLIST.md).

### Added

- **Local history:** `L` lists the 200 most recent commits reachable from
  HEAD. `Enter` lists a commit's files against its first parent, in the
  usual side-by-side view, and `Esc` returns to an unchanged working-tree
  view.
- **`diffkit show <COMMIT>`:** plain-text commit metadata and changed
  files. Unknown, malformed, or non-commit revisions are reported with
  exit code 1.
- **Operations in progress:** merge, rebase (sequencer and apply
  backends, with step counts), `git am`, cherry-pick, revert, and bisect,
  per worktree. Shown in the header and in `diffkit inspect`.
- **Conflict details:** Git's conflict kind, the base/ours/theirs index
  stages (missing stages shown as missing), and whether a working-tree
  file exists. Taken from the existing `git status` call; no index access.
  `inspect` gains a "CONFLICT STAGES" section.
- **Submodule state:** checked out, not initialized, missing, or not a
  directory, plus Git's report on new commits, modified content, and
  untracked content.
- **Type change labels** such as `symlink → regular file`.
- **Terminal restoration on SIGTERM, SIGHUP, and SIGINT** (Unix). The exit
  code is 128 + the signal number; a second signal exits at once.
- **Consistency guard:** a reconciliation starts over (up to 3 times) when
  the index, HEAD, the current branch ref, `packed-refs`, or operation
  state changes while it reads. A result that never settled is not
  published; the worker runs again when Git is idle. The `D` overlay
  counts restarts and withheld results.
- Bisect state files are watched.
- `examples/bench_release.rs`.
- `CHANGELOG.md`, release notes, release checklist, and install,
  platform, security, and troubleshooting sections in the README.

### Changed

- Version 1.0.0; `rust-version = "1.88"` declared.
- Release profile uses LTO and one codegen unit: the macOS arm64 binary
  shrank from 1,791,024 to 1,377,392 bytes.
- The header names the upstream's source: "remote-tracking ref, as last
  fetched" or "local branch", replacing "local ref, not fetched".
- Error messages printed by the CLI escape control characters (a revision
  or path containing an escape sequence can no longer affect the
  terminal).
- The history list drops dates when it is too narrow to show subjects.

### Library API (for code using the `diffkit` crate)

- `DiffContent::Unmerged` and `Notice::Unmerged` carry a `ConflictInfo`.
- `SubmoduleChange` has a `worktree` field.
- `DiffComparison::Commit` and `DiffFileList::target` exist for commit
  inspection; `DiffComparison::ALL` still lists the four working
  comparisons.
- `FileStatus::stages`, `RepositorySnapshot::operations`, and
  `Reconciled::{retries, settled}` were added.
- `GitError::UnknownRevision` was added.
- `diff::commands::{discovery, patch}` take a `Range`.

## 0.1.0 — development phases 1–4

Repository inspection (`inspect`), the diff engine (`diff-stats`), the
side-by-side terminal interface, and automatic synchronization with a
filesystem watcher. See `docs/PHASE_1_IMPLEMENTATION_REPORT.md` through
`docs/PHASE_4_IMPLEMENTATION_REPORT.md`.
