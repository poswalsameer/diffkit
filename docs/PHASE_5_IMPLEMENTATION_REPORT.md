# Phase 5 Implementation Report — Advanced Git Support, Hardening, v1.0

Date: 2026-11-07.

Environment:
- macOS 26.5 (Darwin 25.5.0, Apple M5, arm64)
- rustc 1.98.1
- Git 2.54.0

Nothing was committed, tagged, or published. Implementation is complete
as described below; **acceptance and release are separate decisions**,
and the release checklist is not all green (section I).

Starting point: Phases 1–4 were verified before any change:
- all five Cargo commands passed;
- 347 tests passed;
- the Phase 4 synchronization pipeline was in place.

Phase 4 had not been committed, so the working tree holds Phase 4 and
Phase 5 changes together relative to the last commit (`44325ec`). Section
B lists what Phase 5 itself changed.

---

## A. Summary

### Implemented

- **Conflicts:**
  - Git's conflict kind and the base/ours/theirs index stages (missing
    stages stay missing), taken from the porcelain v2 `u` records the
    status call already returns;
  - working-tree presence;
  - a conflict panel, a header count, and a "CONFLICT STAGES" section in
    `diffkit inspect`.
- **Operations in progress, per worktree:**
  - merge;
  - rebase: sequencer or apply backend, step, branch, onto;
  - `git am`, cherry-pick, revert, bisect.

  Shown in the header and in `inspect`.
- **Submodules:** the working-directory state (checked out, not
  initialized, missing, not a directory) and Git's report on their
  contents, with explanatory text.
- **Local history:**
  - `L` lists the 200 most recent commits on HEAD with full metadata;
  - `Enter` lists a commit's files against its first parent, in the
    existing side-by-side view;
  - `Esc` returns to the unchanged working-tree view.
- **`diffkit show <COMMIT>`:** plain-text commit metadata and file list.
- **Hardening:**
  - SIGTERM, SIGHUP, and SIGINT restore the terminal (Unix);
  - a reconciliation consistency guard: retry when the index, HEAD, or
    refs move during a read, and never publish a result that never
    settled;
  - CLI error messages are escaped.
- **Release:**
  - version 1.0.0, `rust-version`, an LTO release profile;
  - changelog, release notes, release checklist;
  - README install, platform, security, and troubleshooting sections.

### Improved

- Type changes are described in words (`symlink → regular file`).
- Nested repositories and submodules explain why their contents are not
  shown.
- The header names the upstream source ("remote-tracking ref, as last
  fetched" or "local branch").
- The sidebar no longer offers `r` where refreshing cannot help.
- Narrow history lists drop the date to keep the subject visible.
- Bisect state files are watched.

### Intentionally excluded

- Fetching or any network access.
- Any Git write: resolving conflicts, initializing submodules, checking
  out.
- A history graph, branch picker, search, or ranges.
- Configurable history length (bounded at 200).
- Recursion into submodules.
- Distinguishing `git rebase` from `git rebase -i` (Git's state files are
  identical since 2.26).
- Prebuilt binaries for platforms that could not be built or run here.
- A self-updater or background service.

---

## B. Architecture

### Modules changed

**New files:**
- `src/git/state.rs` (operations);
- `src/git/history.rs` (commits, revision resolution);
- `src/app/show.rs` (`diffkit show`);
- `src/tui/ui/history.rs` (history list, commit details, commit bar);
- `examples/bench_release.rs`.

**Modified:**
- `src/git/{models,status,snapshot,errors,mod}.rs`
- `src/diff/{models,commands,engine}.rs`
- `src/sync/{reconcile,scheduler,events,classifier}.rs`
- `src/tui/{app,actions,keymap,terminal,diff_view}.rs`
- `src/tui/ui/{mod,diff_panel,sidebar,header,status,overlays,theme}.rs`
- `src/app/{cli,report,diff_stats,mod}.rs`
- `examples/bench_diff.rs` (new argument type)
- `Cargo.toml`, `Cargo.lock`

### New abstractions

| Abstraction | Role |
|---|---|
| `ConflictStage`, `ConflictStages`, `ConflictInfo` | Stages from status; `DiffContent::Unmerged(ConflictInfo)` |
| `Operation`, `RebaseBackend` | `RepositorySnapshot::operations` |
| `SubmoduleWorktree` | `SubmoduleChange::worktree` |
| `history::{CommitInfo, Signature, History}` | Commit metadata |
| `DiffComparison::Commit`, `DiffFileList::target`, `commands::Range` | Commit diffs on the Phase 2 engine |
| `tui::app::{HistoryState, CommitView}` | The history screen: its own selection, scroll, and cache |
| `sync::reconcile::StateMarker`, `Reconciled::{retries, settled}` | Consistency guard |
| `tui::terminal::Signals`, `TuiError::Signal` | Signal routing |
| `GitError::UnknownRevision` | Invalid user revisions |

### Git commands introduced (all read-only)

- `git log --no-show-signature --no-color --no-notes --no-mailmap
  --format=<NUL-separated fields> --max-count=N --end-of-options <rev> --`
- `git diff-tree -r <base> <commit>`, with the same options as the other
  diff commands (`--no-ext-diff --no-textconv --no-color --find-renames
  --find-copies --submodule=short …`)
- `git rev-parse --verify --quiet --end-of-options <rev>^{commit}`

### Dependencies

`signal-hook = "0.3.18"`, Unix only. It was already in `Cargo.lock` as a
Crossterm dependency at that version, so no new crate is built. No Git
library, async runtime, or other dependency was added.

### How Phases 1–4 were preserved

- **Phase 1:** one status call, no new Git process. Conflict stages come
  from records that were already parsed; operations come from file checks.
- **Phase 2:** commit diffs reuse discovery, the parser, alignment,
  binary handling, and staleness checks unchanged. Only the argument
  builder gained a commit range.
- **Phase 3:** one diff panel renders both the working tree and commits,
  through screen-aware accessors (`active_file`, `loaded`, `view`,
  `scroll`). The working-tree selection, scroll, and cache are never
  changed by the history screen.
- **Phase 4:** synchronization keeps running while history is open, and
  `apply_reconciled` updates only the working-tree state. The guard sits
  inside `reconcile`, so startup, manual refresh, and background sync
  share it.
- **Public API changes,** each needed for a feature:
  - `DiffContent::Unmerged` carries data;
  - `SubmoduleChange::worktree`;
  - `DiffFileList::target`;
  - `FileStatus::stages`;
  - `RepositorySnapshot::operations`;
  - `Reconciled::{retries, settled}`;
  - `commands::{discovery, patch}` take a `Range`.

  Listed in `CHANGELOG.md`.
- **Engineering-rule exceptions,** now written down in
  `ENGINEERING_RULES.md`:
  - operation detection reads Git's state files, because no
    machine-readable Git command reports operations;
  - the guard reads metadata only to decide whether to read again.

---

## C. Feature verification

Legend:
- **Implemented + Tested** — automated tests pass on macOS;
- **Partial** — part is missing, with the reason;
- **Not implemented** — with the reason.

### Workstream A — advanced Git support

| Item | Status | Evidence |
|---|---|---|
| A1 Unmerged paths detected, stages read without touching the index | Implemented + Tested | `advanced_conflicts::every_conflict_shape_reports_exactly_the_stages_git_recorded` compares with `git ls-files --unmerged`, inside a fingerprint |
| A1 Base/ours/theirs, missing stages (add/add, modify/delete both ways) | Implemented + Tested | Same test: `UU`, `AA`, `UD`, `DU`; unit test covers all 7 shapes |
| A1 Distinct from ordinary modifications; explicit in list and panel | Implemented + Tested | `conflicts_are_distinct_from_ordinary_changes`, `interface_explains_each_conflict_and_never_resolves_it` |
| A1 Never resolved automatically | Implemented + Tested | Fingerprints unchanged; no write command exists (section F) |
| A2 Git-reported renames, both paths, with and without edits, across directories | Implemented + Tested | `advanced_renames::rename_across_directories_with_and_without_edits`, plus Phase 2 tests |
| A2 Unusual names; no invented renames | Implemented + Tested | `renames_with_unusual_names_keep_both_paths_exact`, `unrelated_deletion_and_addition_are_not_a_rename`, `case_only_rename_is_reported_as_git_records_it` |
| A3 Gitlinks; changed commit | Implemented + Tested | `advanced_submodules::moved_submodule_commit_is_a_submodule_change_not_a_file` |
| A3 Uninitialized and missing submodules | Implemented + Tested | `uninitialized_submodule_is_explained`, `missing_submodule_directory_is_explained` |
| A3 No recursion; never initialized or modified | Implemented + Tested, with a note | Diffkit lists no files inside. Git itself runs `status` inside *initialized* submodules to report dirtiness (`S.M.`); this is Git's default and is documented |
| A4 Symlink target changes, broken links, no following, deletion, replacement, look-alike files | Implemented + Tested (Unix) | `advanced_symlinks` (4 tests). Windows symlinks not tested |
| A5 Detached HEAD, unborn branch | Implemented + Tested | `repository_operations::detached_and_unborn_head` |
| A5 Merge, rebase, cherry-pick, revert, bisect | Implemented + Tested | 6 tests in `repository_operations` with real Git operations |
| A5 Rebase `-i` vs plain | Partial (by design) | Git 2.26+ writes identical state; reported as the sequencer backend |
| A5 Unmerged index | Implemented + Tested | Header count, `inspect` stages |
| A5 Missing or inaccessible metadata | Implemented + Tested | `missing_git_metadata_is_a_clear_error`, `unreadable_git_directory_is_an_error_not_a_crash`; Phase 4 tests removal during a session |
| A5 Worktree-specific state, `.git` files, git dir vs common dir | Implemented + Tested | `operations_are_per_worktree`, `platform_paths::linked_worktree_under_a_non_ascii_path`, Phase 4 worktree tests |
| A6 Nested repositories conservative | Implemented + Tested | `untracked_nested_repository_is_not_listed_as_files`, `embedded_repository_added_as_gitlink` |

### Workstream B — local commit inspection

| Item | Status | Evidence |
|---|---|---|
| B1 Commit metadata (full and short hash, message, author name/e-mail/date, committer date, parents) | Implemented + Tested | `history::commit_metadata_matches_git` |
| B1 Malformed hashes, missing objects, invalid refs | Implemented + Tested | `invalid_revisions_are_reported_not_misread` (9 invalid, 4 valid) |
| B2 Added, modified, deleted, renamed, binary, mode, submodule in a commit | Implemented + Tested | `commit_diff_covers_every_change_kind` |
| B2 Reuses the diff and detail architecture; read-only | Implemented + Tested | `DiffComparison::Commit` through `load_file_diff`; fingerprints |
| B3 Bounded recent history, select and inspect | Implemented + Tested | `recent_history_is_bounded_newest_first_and_works_detached`, `tui_history` (6) |
| B3 Configurable length | Partial | Bounded at 200; not configurable (allowed by the spec: "configurable or sensibly bounded") |
| B3 Shallow repositories, no commits, detached HEAD | Implemented + Tested | `shallow_clone_marks_its_boundary`, `empty_merge_and_shallow_boundary_commits_are_explained`, `detached_and_unborn_head` |
| B4 Ref freshness wording | Implemented + Tested | Header: "remote-tracking ref, as last fetched" / "local branch" (`tui_render` test); `inspect` already said "Remote state has not been verified by fetching" |

### Workstream C — TUI polish

| Item | Status | Evidence |
|---|---|---|
| C1 Explanations for every special state | Implemented + Tested | Conflicts, binary, symlinks, submodules, renames, deleted, empty, large, unreadable, stale (Phase 2/3), detached, unborn, missing metadata; tests listed above and in Phase 3 |
| C2 Selection, mode, and scroll survive refresh | Implemented + Tested | Phase 4 `tui_sync` |
| C2 Hunk navigation with special files | Implemented + Tested | `tui_states::navigation_on_views_without_hunks_is_harmless` |
| C2 Commit inspection keeps the working-tree view; returning restores it | Implemented + Tested | `inspect_a_commit_and_return_to_an_unchanged_working_tree_view`, `synchronization_while_history_is_open_updates_only_the_working_tree` |
| C2 No stale content shown as current | Implemented + Tested | Phase 4 cache revalidation; unsettled results are withheld (`sync_scheduler::unsettled_results_are_withheld_and_rerun_in_full`) |
| C3 Normal exit, startup errors, panics | Implemented + Tested | PTY tests, `tui_panic` |
| C3 Quit from help | Implemented + Tested (key mapping) | `keymap::overlays_capture_keys` (`q` in help → quit) |
| C3 Resize, very narrow, very short | Implemented + Tested (in-process) | `Action::Resize` tests at sizes down to 30×6; no real-PTY resize was driven |
| C3 Interruption | Implemented + Tested (Unix) | `tui_cli::termination_signals_restore_the_terminal`: SIGTERM, SIGHUP, and SIGINT → 143/129/130, terminal restored, exit < 3 s. SIGKILL cannot be handled (documented) |
| C4 Escape sequences, controls, bidi, zero-width, invalid UTF-8, long names | Implemented + Tested | `tui_states` (3), `tui_history::commit_text_is_escaped`, `history::show_command_prints_metadata_and_files` (CLI escapes), Phase 3 tests |

### Workstream D — cross-platform

| Item | Status | Evidence |
|---|---|---|
| D1 macOS: Unicode, spaces, non-ASCII directories, permissions, symlinks, case-insensitivity, watcher, large trees | Implemented + Tested | `platform_paths`, `advanced_symlinks`, `case_only_rename…` (APFS is case-insensitive here), `unreadable_git_directory…`, benchmarks at 50,000 files |
| D1 Linux, Windows | Not verified | No machine available; compile and lint only |
| D2 Terminal behavior | Partial | macOS pseudo-terminals via `script(1)` only. No human session in Terminal.app or iTerm, and no Linux or Windows terminal |
| D3 Linked worktrees | Implemented + Tested (macOS) | Discovery, HEAD, common dir, index, refs, watching, refresh (Phase 4 and Phase 5 tests) |

### Workstream E — performance, reliability, regression

| Item | Status | Evidence |
|---|---|---|
| E1 Benchmarks for all 7 scenarios | Implemented, measured | Section E |
| E2 Rapid edits, simultaneous events, events during reconciliation | Implemented + Tested | Phase 4 tests and `bench_release` burst |
| E2 Index, branch, and HEAD changes during reconciliation; no partial state | Implemented + Tested | `sync_consistency` stress test, scheduler test |
| E2 Overflow, watcher failure, deletion or relocation, worktree metadata | Implemented + Tested | Phase 4 tests |
| E3 Git subprocess audit | Done | Section F |
| E4 Regression suite | Done | 412 tests; none removed or weakened (section D) |

### Workstream F — release engineering

| Item | Status | Evidence |
|---|---|---|
| F1 CLI: `diffkit`, `PATH`, `inspect`, `diff-stats`, `--help`, `--version`, plus `show` | Implemented + Tested | Existing CLI tests unchanged and passing; new `show` tests; exit codes 0/1/2 and 128+N |
| F2 Build artifacts | Partial | Section G: macOS arm64 built and run; macOS x86_64 built, not run; Linux and Windows compile and lint but cannot link here |
| F3 Installation docs | Implemented; verified on macOS only | README "Install"; `cargo install` into a scratch root |
| F4 Documentation | Implemented | README, CHANGELOG, release notes, checklist, all docs updated |
| F5 Release checklist | Implemented | `docs/RELEASE_CHECKLIST.md`: not ready (section I) |

---

## D. Testing

`cargo test --all-targets`: **412 passed, 0 failed, 0 ignored.**

| | Phase 4 end | Phase 5 end | New |
|---|---:|---:|---:|
| Unit (`src/lib.rs` + `src/main.rs`) | 126 | 138 | 12 |
| Integration (`tests/`) | 221 | 274 | 53 |
| Total | 347 | 412 | 65 |

New integration tests, by file:

| File | Tests |
|---|---:|
| `advanced_conflicts` | 5 |
| `advanced_renames` | 5 |
| `advanced_submodules` | 5 |
| `advanced_symlinks` | 4 |
| `history` | 8 |
| `repository_operations` | 11 |
| `tui_history` | 6 |
| `tui_states` | 3 |
| `platform_paths` | 2 |
| `sync_consistency` | 1 |
| `sync_scheduler` (added) | +1 |
| `tui_cli` (added) | +2 |

**Existing tests preserved.** No test was deleted, ignored, or weakened.
Changed assertions:
- `diff_special_files::conflicts_are_explicit_in_staged_and_unstaged`:
  `assert_eq!(content, Unmerged)` became a match that also requires
  `BothModified`, all three stages, and working-tree presence (stronger).
- `tui_app`: `Notice::Unmerged` became `Notice::Unmerged(_)` (the type
  changed).
- `tui_render::header_shows_tracking_and_detached_states`: the new
  freshness wording.
- `tui_cli::interactive_sync_runs_only_read_only_git_commands`: added
  history steps, `log` and `diff-tree` in the allowed list, and argument
  checks (stronger).

**Read-only proof.** `common::assert_read_only` fingerprints every file
and symlink, `.git` included, around inspection. It is used in the
conflict, rename, submodule, symlink, operation, history, and history-UI
tests.

**Failures during development and their resolution:**

- **Code changes:**
  - `cli_inspect::conflicts_are_listed` failed when stage details were
    added to the existing conflict lines. The original lines were
    restored, and the stages moved to a separate section; existing output
    is unchanged.
  - `tui_render::binary_conflict_mode_rename_and_empty_notices` failed on
    a changed conflict heading and sidebar code. The original heading and
    code were restored, and the kind added on a new line.
  - Plain `git rebase` was reported as interactive (Git writes
    `rebase-merge/interactive` for both). The model now reports the
    backend only.
  - The consistency stress test showed that unsettled results are often
    truly mixed (12–18 of about 32 per run). They are now withheld
    instead of published.
- **Fixtures and expectations:**
  - `git mv` needs an existing destination directory;
  - `git add -A` undid `update-index --chmod`;
  - "two"/"TWO" are too dissimilar to pair as one row;
  - an interactive rebase counts the `exec` line as a step.
- **Test harness:** wide characters occupy two buffer cells; PTY
  transcripts have gaps where differential redraws skip unchanged cells.
  One PTY needle failed intermittently ("(root  ommit)"), so a gap-tolerant
  matcher was added, together with a negative check.
- **Clippy:** one `manual_is_multiple_of`, fixed in code. One
  `allow(clippy::too_many_arguments)` written during development was
  replaced by a struct; no lint is suppressed.

**Repetition.** These timing-sensitive tests were run 6 times in a row
with no failures (48 tests per run):
- `sync_consistency`, `repository_operations`, `platform_paths`;
- `tui_cli`, `sync_scheduler`, `tui_history`, `sync_watch`.

**Platform-specific results:**
- **macOS arm64:** all 412 tests ran.
- **Unix only** (skipped on Windows by `cfg`): the `advanced_symlinks`
  tests, the PTY tests, the unreadable-directory test, and the
  `show` audit.
- **Linux and Windows:** not run.

---

## E. Performance

All numbers were measured on the machine above with the new LTO release
profile; medians. Full tables: `docs/PERFORMANCE_BUDGET.md`, "Phase 5".

| Scenario | Measure | Result |
|---|---|---|
| 1. Small, 100 files (`bench_tui`) | `App::open` / first diff | 19.70 ms / 3.25 ms (Phase 3: 26.66 / 3.92) |
| 1. Small (`bench_inspect`) | `diffkit inspect` end to end | 11.38 ms (Phase 1: 10.29) |
| 2. Medium, 5,000 | `App::open` / first diff | 40.25 / 3.77 ms (Phase 3: 40.47 / 3.75) |
| 3. Large, 50,000 | `App::open` / first diff | 305.87 / 6.07 ms (Phase 3: 306.81 / 5.84) |
| 4. 1,000 changed of 5,000 | open / first frame / first diff / full refresh | 96.4 ms (6 Git) / 0.18 ms / 3.84 ms (1 Git) / 96.3 ms (6 Git) |
| 4. Cache | walk 1,000 files twice | second pass 100% hits; 1,000 entries, peak 3.35 MiB, 0 evictions; at 256 KiB: 2,000 misses, 2,000 evictions |
| 5. 200,000-line file | load / rows / frame | 585.93 ms / 8.81 ms / 0.208 ms (Phase 3: 573.47 / 7.94 / 0.207) |
| 6. Burst, 1,000 writes | events → runs → latency | 2,064 events → 1 reconciliation → 341 ms after the last write |
| 7. Three linked worktrees | open / shared-ref propagation | 36.7 ms (7 Git) / 242 ms |
| History, 1,001 commits | list 200 / open commit / load file | 4.4 ms / 3.3 ms / 3.5 ms (1 Git each) |

Memory (`/usr/bin/time -l`, whole benchmark processes):

| Benchmark | Max RSS | Peak footprint |
|---|---:|---:|
| `bench_release` | 19,709,952 B | 12,796,360 B |
| `bench_tui` | 67,747,840 B | 31,457,712 B |

**Regressions.**
- No TUI regression: startup, first load, frames, and Git process counts
  are unchanged.
- `inspect` is about 1.1–1.5 ms slower on small and medium repositories,
  and Git calls are unchanged (3). Plain `git status` was also slower in
  this session, and an interleaved LTO/no-LTO A/B showed no difference
  (11.91 vs 12.02 ms), so the cause was not isolated. It is reported
  unexplained, not as a measured regression.
- Large/clean `inspect` (+35 ms) tracked `git status`, which was 25 ms
  slower in the same run.
- No optimization was attempted without an identified bottleneck.

---

## F. Read-only verification

### How it was verified

1. **Byte-for-byte fingerprints** of every file and symlink (content and
   mtime), `.git` included, before and after inspection:
   - all new feature tests (`common::assert_read_only`);
   - the Phase 1, 2, and 4 fingerprint tests (`read_only`,
     `diff_read_only`, `sync_watch`), with a positive control showing that
     plain `git status` would be detected.
2. **Logging `git` wrappers** first on `PATH` record every Git process:
   - `diffkit inspect` (`read_only`);
   - `diff-stats` (`diff_read_only`);
   - `show` (`history::show_runs_only_read_only_git_commands`);
   - the running interactive viewer, including watching, manual refresh,
     and history (`tui_cli::interactive_sync_runs_only_read_only_git_commands`).

   Each checks an allow-list, `GIT_OPTIONAL_LOCKS=0`, and that diff
   commands carry `--no-ext-diff --no-textconv` and `log` carries
   `--no-show-signature`.
3. **Source audits:**
   - `only_the_command_layer_starts_processes`: a single
     `Command::new("git")`, in `src/git/commands.rs`;
   - a grep of `src/` for mutating subcommand names (add, commit, reset,
     checkout, switch, restore, merge, rebase, stash, cherry-pick, revert,
     push, pull, fetch, update-index, update-ref, …) found only file-name
     and label strings, no commands.

### Every Git command the application runs

| Command | Used for | Writes | Network | Can Git run configured programs? |
|---|---|---|---|---|
| `rev-parse` (`--absolute-git-dir …`, `@{upstream}`, `--verify --end-of-options <rev>^{commit}`) | Discovery, upstream, revision resolution | No | No | No |
| `status --porcelain=v2 -z --branch --ahead-behind --untracked-files=all` | Snapshot, conflict stages | No (`GIT_OPTIONAL_LOCKS=0` prevents index refresh) | No (`GIT_NO_LAZY_FETCH=1`) | Yes: a `core.fsmonitor` hook, clean filters (e.g. LFS) when re-hashing modified files, and `status` inside initialized submodules |
| `for-each-ref --count=1` | Unborn-HEAD check | No | No | No |
| `diff-index [--cached]`, `diff-files` (`--raw --numstat` or `--patch`, `--no-ext-diff --no-textconv …`) | Working comparisons | No (plumbing never refreshes the index) | No | Clean filters and fsmonitor on working-tree sides; no external diff or textconv |
| `diff-tree -r <base> <commit>` (same options) | Commit diffs | No | No | No working tree involved; no external diff or textconv |
| `ls-files --others --exclude-standard -z` | Untracked files | No | No | fsmonitor possibly |
| `ls-files --others --ignored --exclude-standard --directory -z` | Watch exclusions | No | No | fsmonitor possibly |
| `check-ignore --no-index -- ./…` | Watch exclusions | No | No | No |
| `cat-file -s <oid>` | Binary sizes | No | No | No |
| `log --no-show-signature --no-color --no-notes --no-mailmap --format=… --max-count=N --end-of-options <rev> --` | History, `show` | No | No | No (signature verification disabled) |

Diffkit itself reads, without a Git process:
- working-tree files, for untracked content;
- Git's operation state files;
- file metadata, for the consistency guard and the watcher.

It writes nothing.

**Confirmation.** No automatic network access and no Git mutation were
introduced. Diffkit cannot stop the repository-configured helpers listed
above from running when Git invokes them. They are documented in the
README and in `ENGINEERING_RULES.md` rather than claimed to never run.

---

## G. Cross-platform verification

| Platform | Compile | Test | Runtime | Notes |
|---|---|---|---|---|
| macOS 26.5, arm64 | Yes: `build --release`, `clippy -D warnings` | **412/412 passed** | **Yes, automated:** real-PTY sessions (`script(1)`) for navigation, history, sync, signals, and Git audit; in-process rendering at many sizes | No human session in a terminal emulator |
| macOS, x86_64 | Yes: `clippy --target x86_64-apple-darwin`; release build links (Mach-O x86_64) | No | **No:** Rosetta is not installed, so the binary cannot run here | Build only |
| Linux x86_64 | Yes: `clippy --target x86_64-unknown-linux-gnu` | No | No | `build --release` fails at link: no Linux linker or sysroot on this Mac |
| Linux ARM64 | Yes: `clippy --target aarch64-unknown-linux-gnu` | No | No | Same link limitation |
| Windows x86_64 | Yes: `clippy --target x86_64-pc-windows-msvc` | No | No | `build --release` fails at link: `link.exe` (MSVC) not available |

The `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and
`x86_64-apple-darwin` standard-library targets were added to the local
rustup toolchain for these checks; this changed nothing in the repository.

Compilation is not runtime verification. Linux (inotify) and Windows
(ReadDirectoryChangesW, console input, path handling) behavior is
unverified.

---

## H. Known limitations

**Platforms and verification**
- Linux and Windows have not been run.
- macOS Intel was built but not run.
- No human has worked through the manual checklist in a real terminal
  emulator.
- Minimum versions are declared, not tested: Rust 1.88 (from the
  dependencies) and Git 2.31; only Git 2.54 and rustc 1.98.1 were used.

**Behavior**
- History lists 200 commits from HEAD only. A merge is compared with its
  first parent.
- Plain and interactive rebases are not distinguished.
- Git may run configured helpers (fsmonitor, clean/smudge filters such as
  LFS, `status` inside initialized submodules) during Diffkit's reads.
- SIGKILL cannot restore the terminal. Windows console close events are
  not handled.
- Under continuous Git activity, the screen keeps the last consistent
  state ("◌ Changes detected") until Git is idle; this is deliberate.

**Carried over from Phase 4**
- Linux inotify watch limits; network filesystems.
- Global `core.excludesFile` changes are not watched.
- The selected diff loads on the interface thread.
- The input thread wakes every 200 ms.
- Cache sizes are estimates.

**Release**
- No license file.
- `cargo audit` not run.
- No published binaries, checksums, or signing.

---

## I. Release readiness

| Area | Ready? | Notes |
|---|---|---|
| Build verification | Partial | macOS only; Linux and Windows cannot link here |
| Test verification | Yes (macOS) | 412/412, repeated runs stable |
| Cross-platform runtime | **No** | Only macOS arm64 was run |
| Installation verification | Partial | `cargo install` on macOS only |
| Documentation review | Yes | README, CHANGELOG, release notes, checklist, docs |
| Dependency audit | Partial | No new crates; `cargo audit` not run |
| Security review | Yes | Section F; escaping tests; symlink tests |
| Known limitations | Yes | Section H and README |
| Release artifacts | **No** | Only macOS builds; no checksums, signing, or tag |
| License | **No** | Owner decision needed |

**Assessment: not ready for a v1.0 release.** The code is feature-complete
for the Phase 5 scope and verified on macOS. Before release:
- run the suite and a manual session on Linux and Windows (and macOS
  Intel);
- complete the human checklist;
- choose a license;
- run a vulnerability scan;
- produce and verify artifacts per platform.

Implementation completion is not acceptance; that decision is the
project owner's.

---

## Final validation (exact outcomes)

Run after the last code change:

| Command | Outcome |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check --all-targets` | exit 0, no warnings |
| `cargo build --release` | exit 0, no warnings |
| `cargo test --all-targets` | exit 0; 412 passed, 0 failed, 0 ignored |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| Clippy with `--target` x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-apple-darwin | exit 0 each |
