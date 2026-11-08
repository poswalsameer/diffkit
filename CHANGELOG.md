# Changelog

All notable changes to Diffkit. Versions follow semantic versioning.

## 1.0.0 — unreleased

Not tagged or published. Release readiness is tracked in
[docs/RELEASE_CHECKLIST.md](docs/RELEASE_CHECKLIST.md).

### Performance

- Refreshes ask Git only about the paths `git status` reported, build the
  untracked list from status, reuse repository discovery and ahead/behind
  counts when only files changed, and stop a refresh as soon as Git state
  moves under it. Measured on 20,000 files: 76 → 44 ms per file-triggered
  refresh, and 690 → 154 ms when many files have new timestamps but
  unchanged content.
- Diffs load on a background thread; the interface keeps responding and
  shows "Loading…" during a slow load.
- Line pairing for large replaced blocks is 2–4.6× faster.
- `diffkit diff-stats --load` no longer keeps every diff in memory (peak
  158 → 111 MiB on a 1,000-file change) and no longer searches the loaded
  list for each file.

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
- **Module layout.** Shared data moved to `diffkit::types`
  (`git::models` → `types::repository`, `diff::models` → `types::diff`)
  and shared pure functions to `diffkit::helpers` (`display` →
  `helpers::display`; `strip_newline` and `unquote` → `helpers::git_text`).
  Git path conversion moved to `git::paths`. `LoadedFile` and
  `DiffCollection` are in `diff::engine`. The command line moved into the
  library as `diffkit::cli` (`cli::run`, `cli::args`,
  `cli::commands::inspect`). The re-exports in `diff`, `sync`, and `tui`
  were removed; import each item from its defining module (for example
  `tui::app::App`, `sync::scheduler::SyncService`).
- Every test now lives in `tests/`, grouped into one target per feature.

## 0.1.0 — development phases 1–4

Repository inspection (`inspect`), the diff engine (`diff-stats`), the
side-by-side terminal interface, and automatic synchronization with a
filesystem watcher. See `docs/PHASE_1_IMPLEMENTATION_REPORT.md` through
`docs/PHASE_4_IMPLEMENTATION_REPORT.md`.
