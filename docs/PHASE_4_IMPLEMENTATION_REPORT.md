# Phase 4 Implementation Report — Synchronization and Performance

Date: 2026-11-06.

Environment:
- macOS (Darwin 25.5.0, Apple M5)
- rustc 1.98.1
- Git 2.54.0
- notify 8.2.1 (FSEvents backend)

Phase 5 has not been started. Nothing has been committed or pushed.

## 1. Executive summary

While the interactive viewer is open, Diffkit now keeps itself current
with changes made outside it.

How it works:
- A filesystem watcher (`notify`) feeds a bounded queue.
- One background worker per repository classifies each event into a
  fixed set of change categories.
- The worker debounces them: 200 ms of quiet, and at most 1 s after the
  first event.
- It then reconciles by re-reading the state from Git through the
  existing Phase 1 and Phase 2 APIs.
- Results are applied to the interface in one step. The comparison,
  selected file, scroll position, and still-valid cached diffs are kept.

Git remains the source of truth. Events only decide *when* and *how much*
to re-read; they never build the file list.

Properties:
- Reconciliations never overlap.
- Events that arrive during a run are kept for the next one.
- A burst of 4,000+ watcher events produces one reconciliation of 5 Git
  processes.
- Manual refresh (`r`) goes through the same worker.
- If the watcher cannot start, the viewer shows a warning and keeps
  working with `r`.
- Quitting stops all threads, with bounded waits.

Diff cache: the old fixed 16-entry cache was replaced by a byte-budgeted
LRU cache (64 MiB) that is revalidated against Git metadata, so only diffs
whose files changed are dropped.

Verification:
- All five Cargo commands pass.
- 347 tests pass: 287 from Phases 1–3, plus 60 new.
- Real-watcher tests ran on macOS only.
- Linux and Windows were not run. Windows was linted with Clippy only.
- No human manual testing was performed (section 18).

## 2. Synchronization architecture

```text
notify callback ──try_send──▶ bounded queue (4,096)
                                   │ (full → count drop → OVERFLOW)
              ┌────────────────────▼───────────── thread "diffkit-sync" ──┐
              │ classify → ChangeSet bits → Debouncer → reconcile (Git)   │
              │ → same as before? → publish SyncEvent                     │
              └────────────────────┬──────────────────────────────────────┘
                                   ▼ sink (closure)
input thread ──▶ main-loop channel (bounded 256) ──▶ App::apply_sync → apply_reconciled
                                                     → load_pending → draw
```

Modules, all new under `src/sync/`:

| Module | Lines | Role |
|---|---:|---|
| `events.rs` | 137 | `ChangeSet` bit set, `SyncDiagnostics` |
| `classifier.rs` | 314 | `WatchLayout`, path → category |
| `debounce.rs` | 183 | Quiet-period and maximum-delay debouncer, time injected |
| `reconcile.rs` | 216 | Fresh snapshot and lists with safe reuse; ignored-directory detection |
| `watcher.rs` | 153 | Watch plan, non-blocking handler, start |
| `scheduler.rs` | 508 | Worker loop, `SyncService` handle, restarts, shutdown |
| `cache.rs` | 357 | `DiffCache` LRU with byte budget and revalidation |

Dependency direction: `sync` uses `git` and `diff` and does not know about
`tui`; `tui` uses `sync`.

The worker talks to Git through the `Reconciler` trait. `GitReconciler`
is the real one; tests substitute scripted ones.

## 3. Watcher implementation

- `notify::recommended_watcher` (8.2.1) with
  `Config::default().with_follow_symlinks(false)`. Backends: FSEvents on
  macOS, inotify on Linux, ReadDirectoryChangesW on Windows.
- The callback runs on notify's thread. It only does `try_send` into a
  `sync_channel(4096)`. When the queue is full it increments an atomic
  drop counter and returns, so it never blocks the backend.
- The worker owns the watcher. It is created when the worker starts, and
  rebuilt if the repository layout changes or the watcher reports an
  error.
- There is no polling fallback.

## 4. Git metadata watch strategy

- **Standard repository** (`.git/` inside the working tree): one recursive
  watch on the working tree covers `.git`. Paths inside `.git` are then
  classified (section 6). Objects, logs, hooks, `FETCH_HEAD`, `ORIG_HEAD`,
  and `COMMIT_EDITMSG` are classified as irrelevant.
- **Git directory outside the working tree** (linked worktrees, separate
  Git directories):
  - the per-worktree Git directory is watched non-recursively;
  - the common directory is watched non-recursively;
  - `common/refs` and `common/info` are watched recursively.

  The objects directory is never watched.
- **Ignored directories:** untracked directories that are themselves
  ignored are added to `WatchLayout::excluded`, and events under them are
  dropped. A tracked directory, or a directory whose contents are only
  partly ignored, is never excluded. Candidates come from
  `git ls-files --others --ignored --exclude-standard --directory`, and
  `git check-ignore --no-index` confirms each one.

  This filter is applied after the event arrives. It reduces work, not
  the number of kernel watches (see limitations).

## 5. Worktree support

`WatchLayout::new` uses the Phase 1 repository information:
- `git_dir` is the per-worktree directory, for example
  `main/.git/worktrees/wt`;
- `common_dir` is the shared one.

Classification checks `git_dir` first, so a linked worktree's own HEAD,
index, and merge or rebase state count. The main worktree's HEAD, index,
and merge files inside `common_dir` are ignored, because they do not
affect this worktree. Shared refs, packed-refs, and config affect every
worktree.

Tested by `sync_watch::linked_worktree_and_shared_references`:
- an edit in the linked worktree;
- a commit there;
- a ref moved from the main worktree, which changes the linked
  worktree's behind count to 1.

## 6. Event classification

Each path maps to categories (`ChangeSet`, 12 bits):

| Path | Category |
|---|---|
| `index`, `index.lock`, `sharedindex.*` (own worktree) | INDEX |
| `HEAD` (own worktree) | HEAD |
| `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE`, `MERGE_RR`, `AUTO_MERGE`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `sequencer/` | MERGE_STATE |
| `rebase-merge/`, `rebase-apply/`, `REBASE_HEAD` | REBASE_STATE |
| `refs/**`, `packed-refs`, `shallow` (and `.lock` forms) | REFS |
| `config`, `config.worktree` | CONFIG |
| `info/exclude`, any `.gitignore` | WORKTREE + IGNORE_RULES |
| `modules/*/` HEAD, index, refs, packed-refs | WORKTREE |
| The working-tree root, `.git` itself | STRUCTURE |
| Other working-tree paths | WORKTREE |
| Objects, logs, hooks, FETCH_HEAD, ORIG_HEAD, COMMIT_EDITMSG, other worktrees' state, nested repositories' `.git`, excluded directories, paths outside the watched roots | irrelevant |
| Access events, except a write-close | irrelevant |
| `Flag::Rescan`, watcher errors, dropped events | OVERFLOW |

MANUAL and STARTUP are set by the program, not by events. All of a
rename's paths are classified, and a create–rename atomic save counts as
WORKTREE.

## 7. Debounce strategy and chosen values

- Quiet period: **200 ms** after the last relevant event.
- Maximum delay: **1,000 ms** after the first event of a batch.
- Deadline: `min(last + quiet, first + max)`.

Why these values:
- 200 ms covers an editor's write–rename–metadata sequence and Git's
  lock–write–rename of the index, so one save gives one run.
- 1 s bounds staleness under continuous activity, such as a build
  writing files or a save every 50 ms.

The debouncer does not read the clock itself. The caller passes monotonic
`Instant`s, so unit tests are deterministic. Manual refresh (`r`) skips
the debounce and runs immediately, together with any pending changes.

## 8. Reconciliation lifecycle

1. Take the batch: its categories and event count.
2. Send `Synchronizing`.
3. Read a fresh Phase 1 snapshot: `rev-parse`, `status`, `for-each-ref`.
4. Decide which comparison lists can be reused. Reuse requires that:
   - the HEAD commit is unchanged;
   - the repository locations are unchanged;
   - the previous list was valid.

   Then:
   - Staged (HEAD → index) is reused when the trigger contains only
     WORKTREE, IGNORE_RULES, REFS, or CONFIG.
   - Unstaged, Combined, and Untracked are reused only for REFS or
     CONFIG.
   - INDEX, HEAD, MERGE_STATE, REBASE_STATE, STRUCTURE, OVERFLOW,
     MANUAL, and STARTUP always rediscover every list.
5. Rediscover the other lists with the Phase 2 plumbing commands
   (`diff-index`, `diff-files`, `ls-files`). A failed list is stored as
   an error for that comparison only.
6. If the trigger touched ignore rules, the index, the structure, an
   overflow, or a manual refresh, recompute the ignored-directory
   exclusions and rebuild the watcher if they changed.
7. If the result equals the previous state (`same_state`), send
   `Unchanged`. The interface is not touched.
8. Otherwise send `Reconciled(Box<Reconciled>)`, with `pending` set if
   events arrived during the run.

On error, send `Failed(message)` and keep the previous state. The next
event or `r` retries; there is no automatic retry loop.

## 9. Concurrency model

Threads:
- main (interface)
- `diffkit-input` (polls the terminal with a 200 ms timeout)
- `diffkit-sync` (one per repository)
- notify's internal backend thread

Guarantees:
- **One reconciliation at a time:** runs execute sequentially on the one
  worker thread. Tests track the maximum number of concurrent runs and
  require it to be 1.
- **No lost events:** after each run, the worker drains the queue into
  the debouncer before publishing, so `pending` is accurate and the next
  batch includes everything that arrived meanwhile.
- **No unbounded queues:**
  - the watcher queue holds 4,096 events and counts drops;
  - pending changes are a 16-bit set;
  - the interface channel holds 256 items, with backpressure on the
    sender.
- **Shutdown** (`terminal::run`):
  1. stop and join the input thread;
  2. drop the interface receiver;
  3. `SyncService::shutdown(2 s)` sets a stop flag and sends a wake
     message, then waits up to 2 s. A worker still inside a Git call is
     detached rather than blocking exit.
  4. restore the terminal.

  `Drop` does the same with a 2 s limit.

## 10. Atomic application state updates

`App::apply_reconciled(Reconciled)` replaces the snapshot and all four
lists together, on the interface thread, between frames. No frame shows a
mix of old and new state.

For each comparison, the selection is restored in this order:
1. the same file key (old path, new path);
2. the same new path, so a staged file that moved to another comparison
   keeps its place;
3. the previous index, clamped to the list.

Scroll and hunk positions are kept when the shown file's diff is still
valid. If the selected file disappeared, the nearest file is selected and
a notice says "X is no longer in MODE".

Bug fixed during this work: re-applying state used to reset the scroll
position to the top on every save. `clamp_scroll` now leaves it alone
while no diff is loaded.

## 11. Cache design and invalidation

`DiffCache<V>`:
- **Key:** `CacheKey { comparison, old_path, new_path }`. Renames and
  different comparisons never share entries.
- **Entry:** the `DiffFile` it was loaded from, the value (a `DiffView`
  or a load error), an approximate byte size, and a recency stamp.
- **Revalidation** after every applied reconciliation: an entry survives
  only if Git's fresh `DiffFile` for its key is identical, ignoring the
  per-discovery `FileId`. Compared fields: object IDs, working-tree stamp
  (size and mtime), line counts, modes, kind, binary, and conflict
  status. Anything else is an invalidation.
- **Lazy loading is unchanged:** discovery loads no content. Only the
  selected file is loaded, now through `cache.lookup` before calling Git.

The Phase 3 test `tui_app::refresh_keeps_selection_by_identity` asserted
that refresh empties the cache. It now asserts that the 3 unchanged files
stay cached. The change is intentional and commented in the test.

## 12. Memory budget

- **Cache:** 64 MiB by default (`DEFAULT_BUDGET_BYTES`). It is
  adjustable through `App::set_cache_budget`, with no command-line flag.
- **Entry size:** `DiffView::approx_bytes`, which sums the display rows'
  text and fixed per-row overhead (512 bytes for a failed load).
- **Eviction:** least-recently-used first, until the cache is under
  budget. The diff on screen is protected, so it can exceed the budget
  alone. One 200,000-line diff measured 6.83 MiB.
- **Other bounds:**
  - event queue 4,096;
  - interface channel 256;
  - one `Reconciled` (four `Arc` lists) held by the worker and one by the
    interface.

## 13. Watcher failure recovery

| Situation | Behavior |
|---|---|
| Watcher cannot start (permissions, inotify limit, unsupported filesystem) | `WatcherUnavailable(reason)`; indicator `! Watcher unavailable`; a notice explains that `r` works. A startup reconciliation still runs. |
| Restarts | Up to 3, after 1 s, 5 s, and 30 s. A manual refresh resets the count and tries again. |
| Watcher error event, `Rescan` flag, or full queue | OVERFLOW: full reconciliation, exclusions recomputed, watcher rebuilt. Counted in diagnostics. |
| Repository moved or deleted | Reconciliation fails → `! Refresh required`; the last good state stays on screen. |
| `--no-watch` | No watcher; indicator `○ Auto-sync off`; `r` uses the worker. |

## 14. TUI integration

- **Status bar:** a right-aligned indicator:
  - `● Synced`
  - `◌ Changes detected`
  - `↻ Synchronizing`
  - `! Watcher unavailable`
  - `! Refresh required`
  - `○ Auto-sync off`
- **Event loop:** blocks on one channel carrying input and sync results.
  There is no busy loop and no Git on the interface thread, except when
  loading the selected diff (unchanged from Phase 3).
- **Keys:**
  - `r` asks the worker to refresh;
  - `D` toggles the diagnostics overlay (inside it: Esc or `D` closes,
    `q` quits, `r` refreshes).
- **Command line:** `diffkit --no-watch [PATH]`. The existing `inspect`
  and `diff-stats` commands and their output are unchanged.
- **Help overlay:** lists `r` and `D`.

## 15. Diagnostics

Diagnostics appear in a `D` overlay inside the viewer, not printed into
the screen, not logged, and with no new dependency. There was no existing
diagnostic command for the interactive viewer, and the plain-text
`inspect` and `diff-stats` commands run without a watcher, so they had
nothing to report.

The overlay shows:
- filesystem events, relevant events, coalesced events, dropped events,
  and overflows;
- reconcile requests, completed, unchanged, and failed;
- lists reused;
- last and average reconcile time;
- Git processes;
- watcher state, errors, and restarts;
- cache hits, misses, evictions, invalidations, entries, approximate
  bytes and peak, and the budget.

## 16. Performance benchmarks

Command: `cargo run --release --example bench_sync -- 5`.
- 1 warm-up run and 5 measured runs per scenario; medians.
- Real watcher, 200 ms quiet period.
- Latency includes the 200 ms quiet period.
- Full tables and method: `docs/PERFORMANCE_BUDGET.md`, "Phase 4".

| Tracked files | One save | 1,000-write burst | Save every 50 ms for 3 s | `git add` | Branch switch | Manual refresh |
|---:|---|---|---|---|---|---|
| 100 | 1 run, 5 Git, 42.7 ms, latency 258 ms | 4,124 events → 1 run, 5 Git, 269 ms | 224 events → 3 runs | 1 run, 7 Git, 265 ms | 1 run, 6 Git, 256 ms | 7 Git, 53 ms |
| 5,000 | 1 run, 5 Git, 57.9 ms, 272 ms | 4,172 → 1 run, 273 ms | 3 runs | 7 Git, 279 ms | 6 Git, 278 ms | 69 ms |
| 50,000 | 1 run, 5 Git, 181.9 ms, 398 ms | 4,128 → 1 run, 394 ms | 3 runs | 7 Git, 425 ms | 6 Git, 396 ms | 210 ms |

For continuous saves, the measured latency (0 ms) is not meaningful: the
final state had already been published by a maximum-delay run. It is
reported as not applicable.

Cache (5,000 files, 200 changed):
- Two-file back and forth: 99 hits / 2 misses, 1 Git call.
- Second pass over 200 files: 0 Git calls, 100% hits, 0.45 MiB.
- 256 KiB budget: 289 evictions, peak 256 KiB, and 200 Git calls on the
  second pass.
- Reload after invalidation: 3.55 ms, 1 Git call.
- One large diff: 6.83 MiB.

Whole benchmark process, from `/usr/bin/time -l`:
- maximum resident set size 65,339,392 bytes;
- peak footprint 33,063,368 bytes.

Comparison with earlier phases:
- Reconciliation adds no work to loading a diff: a cached file is shown
  with 0 Git calls. Phase 3 kept only 16 diffs and dropped all of them on
  refresh.
- A full refresh at 50,000 files (210 ms) is in line with the Phase 1
  `inspect` measurements plus Phase 2 discovery.

UI responsiveness:
- Measured structurally, not timed precisely:
  `tui_sync::the_interface_stays_responsive_during_reconciliation`
  performs 200 navigation actions with a render after each, while a
  reconciliation is blocked, and requires them to finish under 5 s.
- Phase 3 measured frame cost at about 0.2 ms.

## 17. Automated test results

`cargo test`: **347 passed, 0 failed, 0 ignored**.
- Phase 3 baseline: 287.
- New: 60 (24 unit tests in `src/sync`, 36 integration tests).
- No test was removed. One Phase 3 assertion changed (section 11).

| Area (spec category) | Tests |
|---|---|
| Classification (A) | `classifier` unit ×7 |
| Debounce (B) | `debounce` unit ×6; `sync_scheduler` burst, maximum delay, manual refresh |
| Reconciliation and concurrency (C) | `sync_scheduler` ×8 (no overlap, pending events, failure, overflow, shutdown); `sync_reconcile` ×5 |
| Real Git changes (D) | `sync_watch` ×11: edit, stage/unstage, untracked, delete, rename, commit, branch switch, conflict, resolution, burst, ignored output, removal, linked worktree, read-only fingerprint |
| Cache (E) | `cache` unit ×6; `tui_sync` cache budget and targeted invalidation |
| UI (F) | `tui_sync` ×10: selection, scroll, vanished file, header, indicators, degraded mode, `--no-watch`, diagnostics, responsiveness |
| Terminal and process (G) | `tui_cli` +2 PTY sessions: external edits without `r` and a clean exit under 3 s; Git audit of the running viewer |
| Read-only (H) | `sync_watch::synchronization_never_writes_git_metadata`; the PTY Git audit |

Repeated runs: `sync_scheduler` and `sync_watch` were each run 6 times in a
row, with no failures. Every timing-sensitive test waits for a condition
with a generous deadline, never a fixed sleep.

## 18. Manual testing results

**No human manual testing was performed.** The spec's 20-step checklist
was covered by automated equivalents instead. Each was run against a real
repository and, where noted, through a real pseudo-terminal running the
release binary:

| # | Step | Automated equivalent |
|---|---|---|
| 1–4 | Open, leave running, edit externally, see the update | PTY `external_edits_appear_without_pressing_r` |
| 5–6 | Repeated saves are debounced | `sync_watch` burst (≤ 3 runs for 100 writes); bench |
| 7–9 | Untracked, rename, delete | `sync_watch` |
| 10–13 | Stage, unstage, commit, switch branches externally | `sync_watch`, `tui_sync` header test |
| 14–15 | Conflict and resolution | `sync_watch` |
| 16 | Selection kept | `tui_sync` |
| 17 | Manual refresh | `tui_sync` degraded mode; PTY Git audit |
| 18 | Responsive during sync | `tui_sync` responsiveness |
| 19 | Watcher warning | `tui_sync` indicator and degraded-mode tests (an injected failure, not a real one) |
| 20 | Clean quit | PTY tests: terminal restored, exit under 3 s |

A person should still run the checklist in a real terminal emulator with
a real editor.

## 19. Platform verification

| Platform | Status |
|---|---|
| macOS 26 (Darwin 25.5.0), FSEvents | **Verified:** the full test suite, including the real-watcher and PTY tests, and the benchmark. |
| Linux, inotify | **Not run.** Compiles from shared code only; no Linux machine was available. |
| Windows, ReadDirectoryChangesW | **Not run.** `cargo clippy --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings` passes. That proves only that it compiles. |

## 20. Known limitations

- **Linux inotify:** adds a watch per directory, including ignored ones
  such as `target/` or `node_modules/`, because exclusion happens after
  the event arrives. Large trees can exceed
  `fs.inotify.max_user_watches`; Diffkit then shows "Watcher unavailable"
  and `r` still works.
- **Network and virtual filesystems:** often deliver no events. Use `r`
  or `--no-watch`.
- **Global `core.excludesFile`:** changes to it are not watched. They are
  picked up at the next reconciliation.
- **Snapshot consistency:** a snapshot is several Git calls, not one
  atomic read. A change in the middle sets `pending`, and the next run
  corrects it.
- **Diff loading on the interface thread:** the selected diff still
  loads synchronously, as in Phase 3. Reconciliation itself is off the
  interface thread.
- **Staged-list reuse** assumes every index change produces an index
  event. A missed event is corrected by the next INDEX, OVERFLOW, or
  manual run.
- **Extra startup run:** the worker runs one STARTUP reconciliation right
  after the interface opens. This is redundant when nothing changed, and
  is reported as `Unchanged`.
- **Idle wakeups:** the input thread wakes every 200 ms to check its stop
  flag. The sync worker blocks.
- **Cache sizes are estimates,** not allocator measurements.
- **Helpers Git may run:** Git can run repository-configured helpers while
  reading state, such as clean filters (for example Git LFS) and a
  `core.fsmonitor` hook. Diffkit disables external diff and textconv, but
  cannot prevent these. The read-only guarantee covers Diffkit's own
  commands; helpers are the repository's configuration.
- **Ahead/behind:** Diffkit never fetches, so the counts reflect local
  remote-tracking refs only.

## 21. Dependency changes

- Added `notify = "8.2.1"`, the latest stable release (9.0 is still a
  release candidate). It is the standard cross-platform watcher. It is
  used through its callback API, so no async runtime and no debouncer
  crate are needed.
- New crates in `Cargo.lock`:
  - `notify`, `notify-types`
  - `inotify`, `inotify-sys` (Linux)
  - `kqueue`, `kqueue-sys` (BSD)
  - `objc2-core-foundation`, `objc2-core-services` (macOS FSEvents)
  - `walkdir`, `same-file`, `winapi-util`
- No database, network client, daemon, Git library, or async runtime was
  added.

## 22. Exact files changed

New:
- `src/sync/mod.rs`
- `src/sync/events.rs`
- `src/sync/classifier.rs`
- `src/sync/debounce.rs`
- `src/sync/reconcile.rs`
- `src/sync/cache.rs`
- `src/sync/watcher.rs`
- `src/sync/scheduler.rs`
- `tests/sync_scheduler.rs`
- `tests/sync_reconcile.rs`
- `tests/sync_watch.rs`
- `tests/tui_sync.rs`
- `examples/bench_sync.rs`
- `docs/PHASE_4_IMPLEMENTATION_REPORT.md`

Modified (`git diff --stat HEAD`: 23 files, +1,405 / −117, before this
report):
- `Cargo.toml`, `Cargo.lock`
- `src/lib.rs`, `src/app/cli.rs`, `src/git/commands.rs`
- `src/tui/actions.rs`, `src/tui/app.rs`, `src/tui/diff_view.rs`,
  `src/tui/keymap.rs`, `src/tui/mod.rs`, `src/tui/terminal.rs`
- `src/tui/ui/overlays.rs`, `src/tui/ui/status.rs`
- `tests/tui_app.rs`, `tests/tui_cli.rs`
- `README.md`
- `docs/ARCHITECTURE.md`, `docs/DATA_MODEL.md`,
  `docs/ENGINEERING_RULES.md`, `docs/PERFORMANCE_BUDGET.md`,
  `docs/TECH_STACK.md`, `docs/TESTING_STRATEGY.md`
- `docs/phases/PHASE_4_SYNCHRONIZATION_PERFORMANCE.md`

## 23. Cargo validation results

All run after the last code change:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check` | exit 0, no warnings |
| `cargo build --release` | exit 0, no warnings |
| `cargo test` | exit 0; 347 passed, 0 failed, 0 ignored |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| Same, `--target x86_64-pc-windows-msvc` | exit 0 (lint only) |

**Read-only confirmation:** no repository-mutating command was added.
- The only new Git subcommand is `check-ignore`, which is read-only.
- All Git calls still go through the single spawn site in
  `src/git/commands.rs`, with `GIT_OPTIONAL_LOCKS=0` and
  `GIT_NO_LAZY_FETCH=1`.
- `GIT_LITERAL_PATHSPECS=1` is also set, except for `check-ignore`, which
  rejects it; its paths are prefixed with `./` instead.
- The PTY audit test records every Git process the running viewer starts
  and allows only rev-parse, status, for-each-ref, diff-index,
  diff-files, ls-files, cat-file, and check-ignore.
- The fingerprint test shows `.git` is unchanged byte for byte while
  syncing.
