# Performance Budget

## Targets (engineering targets, not guarantees)

| Repository | Tracked files | Target for `diffkit inspect` |
|---|---:|---:|
| Small | 100 | ≈ 300 ms |
| Medium | 5,000 | ≈ 1 s |
| Large | 50,000 | ≈ 3 s |

## How it is measured

```sh
cargo build --release
cargo run --release --example bench_inspect -- 15   # measured runs per scenario
```

- Repositories are generated deterministically under
  `$TMPDIR/diffkit-bench` (`src/modNNN/fileNNNNNN.rs`, 100 files per
  directory). Each tracks a local bare remote and is one commit ahead.
- After setup the harness waits more than one second and runs
  `git update-index --really-refresh`, so no index entry is "racily clean".
  Diffkit never writes the index, so racy entries would otherwise force
  content re-reads on every run.
- **Clean** scenario: no changes.
  **Dirty** scenario: 2% of tracked files (minimum 10) modified. Half are
  unstaged, a quarter staged, a quarter staged then modified again. Also
  10% of that number deleted, and half that number added as untracked files.
- Each scenario runs 3 warm-up runs and then N measured runs of the release
  binary, end to end: process start, all Git subprocesses, parsing, and
  report output to `/dev/null`. The individual times and the median are
  printed. Raw `git status` with Diffkit's arguments is timed the same way
  as a baseline.
- Memory comes from `/usr/bin/time -l`. "Peak memory footprint" is the
  Diffkit process alone. "Maximum resident set size" is the maximum over
  Diffkit and the Git children it waits for, so it reflects Git.
- Git subprocess counts come from a logging `git` wrapper placed on `PATH`.

## Results — 2026-11-06

Environment: macOS 26.5 (arm64), Apple M5 (10 logical cores), 16 GiB RAM,
Git 2.54.0, rustc 1.98.1, release build. Second benchmark pass (repositories
already built), 3 warm-up + 15 measured runs.

| Repo | Tracked | Scenario | Changed entries | Git calls | diffkit median | git status median | diffkit own peak footprint | Max RSS in tree |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| small | 100 | clean | 0 | 3 | 10.29 ms | 3.52 ms | 1.1 MiB | 8.2 MiB |
| small | 100 | dirty | 16 | 3 | 10.32 ms | 3.56 ms | 1.1 MiB | 8.2 MiB |
| medium | 5,000 | clean | 0 | 3 | 15.59 ms | 8.74 ms | 1.1 MiB | 9.5 MiB |
| medium | 5,000 | dirty | 160 | 3 | 15.90 ms | 9.05 ms | 1.2 MiB | 9.5 MiB |
| large | 50,000 | clean | 0 | 3 | 63.11 ms | 56.17 ms | 1.0 MiB | 18.4 MiB |
| large | 50,000 | dirty | 1,600 | 3 | 66.06 ms | 58.37 ms | 1.7 MiB | 18.4 MiB |

Dirty scenarios in detail: small has 5 staged, 9 unstaged, 3 partially
staged, 5 untracked, and 1 deleted; medium has 50/85/25/50/10; large has
500/850/250/500/100.

Individual measured runs (ms):

- small/clean: 10.28, 10.32, 10.32, 10.40, 10.98, 10.51, 10.31, 10.18, 10.12, 10.27, 10.13, 10.53, 10.29, 10.06, 10.14
- small/dirty: 10.16, 10.10, 10.14, 10.47, 10.32, 10.69, 10.44, 10.18, 10.22, 10.20, 10.67, 10.54, 10.06, 10.67, 10.63
- medium/clean: 15.63, 15.74, 15.77, 15.72, 15.48, 15.50, 15.96, 15.76, 15.50, 15.51, 15.59, 15.21, 16.20, 15.42, 15.48
- medium/dirty: 15.75, 15.90, 15.78, 16.36, 16.98, 16.44, 15.83, 16.30, 16.25, 15.66, 15.82, 15.60, 15.90, 16.06, 15.84
- large/clean: 63.57, 63.22, 63.41, 62.80, 62.95, 63.68, 62.85, 62.78, 62.81, 62.76, 63.11, 63.14, 63.70, 63.49, 62.12
- large/dirty: 65.94, 65.74, 65.40, 66.22, 66.43, 65.96, 65.54, 66.06, 65.43, 66.26, 66.98, 66.02, 66.29, 66.18, 66.64

All six scenarios are well inside their targets on this machine. Diffkit
adds about 7 ms on top of `git status`: its own process start plus two
`rev-parse` subprocesses (discovery and upstream commit).

### Variance and the first pass

The first pass (10 measured runs) began immediately after the 50,000-file
fixture was written. Its large/clean median was 92.97 ms (88.9–94.6 ms),
with raw `git status` at 71.9–85.0 ms in the same window. The rerun above,
on the same repositories, was stable at 62–64 ms. The first pass is treated
as disturbed by post-creation system activity. The exact source (for
example filesystem indexing) was not instrumented.

### First-launch cost of a new binary

The previous report's first small-repository run took 447 ms against about
25 ms afterwards. To isolate it, the harness copies the release binary to a
new file three times and times `--version`, which runs no Git:

| Trial | First launch | Second launch |
|---:|---:|---:|
| 0 | 169.96 ms | 3.14 ms |
| 1 | 125.82 ms | 4.15 ms |
| 2 | 129.16 ms | 3.66 ms |

The first execution of a newly written executable costs about 125–170 ms
on this macOS host, even with no Git work. That matches macOS's
first-run verification of new executables; the subsystem itself was not
traced. This one-time cost per new binary, plus cold caches, accounts for
the 447 ms outlier. It is not inspection time. Warm-up runs now exclude it.

### Not measured

- Linux and Windows.
- Repositories with submodules, very deep histories, or `core.fsmonitor`
  / untracked cache enabled.
- Cold filesystem caches (purging them needs root).

## Phase 2: diff engine — 2026-11-06

```sh
cargo build --release
cargo run --release --example bench_diff -- 7
```

Environment: macOS 26.5 (arm64), Apple M5 (10 logical cores), Git 2.54.0,
release build. In-process library timings with 2 warm-up runs, then the
median of 7 measured runs. Git subprocesses are counted with
`git::commands::thread_invocations`. Memory comes from `/usr/bin/time -l
diffkit diff-stats --load`: "footprint" is the diffkit process alone, and
"tree max RSS" is the largest process including Git children.

Each tracked file has 40 lines. "2% of files" edits that many files, two
lines each, and stages every second one. For the small repository that is
2 files. All runs below use the unstaged comparison.

| Repo | Tracked | Scenario | Changed (unstaged / staged) | Discover | Load 1 file | `load_all` | Git calls (discover / load 1 / load_all) | Footprint / tree max RSS |
|---|---:|---|---:|---:|---:|---:|---|---:|
| small | 100 | one file | 1 / 0 | 3.94 ms | 3.21 ms | 3.16 ms | 1 / 1 / 1 | 1.2 / 8.1 MiB |
| small | 100 | 2% of files | 1 / 1 | 3.89 ms | 3.31 ms | 3.37 ms | 1 / 1 / 1 | 1.3 / 8.2 MiB |
| medium | 5,000 | one file | 1 / 0 | 6.53 ms | 3.45 ms | 6.47 ms | 1 / 1 / 1 | 1.2 / 9.3 MiB |
| medium | 5,000 | 2% of files | 50 / 50 | 8.31 ms | 3.45 ms | 8.70 ms | 1 / 1 / 1 | 2.7 / 9.5 MiB |
| large | 50,000 | one file | 1 / 0 | 35.57 ms | 5.23 ms | 35.56 ms | 1 / 1 / 1 | 1.2 / 19.2 MiB |
| large | 50,000 | 2% of files | 500 / 500 | 51.81 ms | 5.17 ms | 55.13 ms | 1 / 1 / 1 | 9.1 / 19.0 MiB |

Single files, in a repository with 100 other files, unstaged:

| File | Change | Git line counts | Load (median) | Git call | Parse | Hunks + alignment | Rows | Alignment |
|---|---|---:|---:|---:|---:|---:|---:|---|
| large.txt | 200,000 lines, every 100th edited | +2000 −2000 | 521.20 ms | 517.15 ms | 0.71 ms | 1.35 ms | 14,000 | exact |
| blob.bin | 20 MiB binary, 5,120 bytes flipped | binary | 40.47 ms | 37.12 ms | 0.00 ms | 0.00 ms | 0 | exact |
| rewrite.txt | 20,000 lines replaced, no common lines | +20000 −20000 | 59.27 ms | 9.24 ms | 1.07 ms | 49.44 ms | 40,000 | exact |
| partial.txt | 20,000 lines, every 3rd changed | +6667 −6667 | 234.20 ms | 230.30 ms | 0.65 ms | 3.03 ms | 20,000 | exact |

`diffkit diff-stats --load` on the single-file repository loads every file
in all four comparisons. It peaked at a 29.5 MiB footprint for diffkit and
78.5 MiB tree max RSS.

Individual measured runs (ms):

- small/one file: discover 4.18, 3.94, 3.81, 3.67, 3.72, 3.94, 4.73; load 3.73, 3.49, 3.38, 3.21, 3.19, 3.13, 3.18; load_all 3.34, 3.21, 3.22, 3.16, 3.13, 3.15, 3.16
- small/2%: discover 4.02, 4.20, 3.89, 3.82, 4.14, 3.71, 3.59; load 3.31, 3.24, 3.24, 3.45, 3.32, 3.33, 3.16; load_all 3.42, 3.33, 3.33, 3.61, 3.37, 3.42, 3.25
- medium/one file: discover 7.01, 6.92, 6.54, 6.41, 6.38, 6.37, 6.53; load 3.45, 3.62, 3.49, 3.58, 3.42, 3.41, 3.38; load_all 6.45, 6.59, 6.44, 6.46, 6.48, 6.49, 6.47
- medium/2%: discover 8.30, 8.20, 8.31, 8.36, 8.29, 8.38, 8.38; load 3.45, 3.53, 3.43, 3.38, 3.44, 3.45, 3.56; load_all 8.71, 8.55, 8.70, 8.58, 8.98, 8.29, 8.76
- large/one file: discover 35.73, 35.37, 36.14, 35.57, 35.56, 36.02, 35.35; load 5.05, 5.14, 5.80, 5.34, 5.23, 5.24, 5.23; load_all 35.54, 35.72, 35.56, 35.60, 35.56, 35.78, 35.56
- large/2%: discover 51.60, 52.33, 52.07, 50.40, 51.81, 51.52, 51.90; load 4.98, 5.30, 5.29, 5.15, 5.20, 5.07, 5.17; load_all 54.64, 54.11, 55.25, 55.13, 55.34, 55.69, 54.96
- large.txt: 516.46, 520.57, 518.02, 521.20, 522.46, 527.71, 522.49
- blob.bin: 40.74, 40.47, 40.47, 40.28, 40.53, 40.39, 40.69
- rewrite.txt: 59.24, 59.25, 59.27, 59.21, 59.51, 59.70, 59.72
- partial.txt: 234.62, 233.63, 234.98, 234.17, 234.96, 234.19, 234.20

Observations:

- Discovery and loading are one Git process each, regardless of repository
  size or change count. Discovery cost grows with tracked files because
  `diff-files` checks the stat data of every index entry.
- Loading one file costs about the same in every repository (3–5 ms).
  Pathspec limiting works.
- For large text files, almost all time is Git computing the histogram diff
  (517 of 521 ms). Diffkit's parsing and alignment take about 2 ms.
- The no-common-lines rewrite is the worst case for Diffkit itself: 49 ms
  for 40,000 rows. Myers is skipped because no line is shared; the time is
  the bounded pairing pass (at most 16 similarity checks per removed line).
- No scenario reached the alignment edit-cost bound.

Not measured: Linux, Windows, cold caches, debug builds, and repositories
with fsmonitor or untracked cache enabled.

## Phase 3: terminal interface — 2026-11-06

```sh
cargo run --release --example bench_tui -- 7
```

Release build, in-process, 1 warm-up and 7 measured runs (median). Frames
are rendered into a 200×60 Ratatui `TestBackend`, so terminal I/O is not
included.

| Tracked files | Changed (unstaged) | `App::open` | Git calls | First diff load | Git calls | Total to first diff |
|---:|---:|---:|---:|---:|---:|---:|
| 100 | 2 | 26.66 ms | 6 | 3.92 ms | 1 | 30.58 ms |
| 5,000 | 100 | 40.47 ms | 6 | 3.75 ms | 1 | 44.22 ms |
| 50,000 | 1,000 | 306.81 ms | 6 | 5.84 ms | 1 | 312.64 ms |

`App::open` is the Phase 1 snapshot (rev-parse and status) plus discovery
of all four comparisons. No diff content is loaded.

Large diff: a 200,000-line file with every 100th line edited.

| Measure | Result |
|---|---:|
| Phase 2 `load_file_diff` alone | 573.47 ms |
| Building display rows (`DiffView::new`) | 7.94 ms |
| TUI first load (both; single run) | 565.93 ms |
| Display rows | 16,000 |
| One frame | 0.207 ms |
| PageDown or `]`, then one frame | 0.222 ms, 0 Git calls |
| One frame at the bottom (single run) | 0.219 ms |
| Returning to a previously loaded file | 0.000 ms, 0 Git calls |

Compared with Phase 2, the interface adds only display-row construction
(about 8 ms for 16,000 rows) on top of the engine's load. Frame cost does
not depend on diff size, because only visible rows are built. Startup adds
discovery of the three comparisons not on screen (metadata only), so their
file counts can be shown in the tab bar.

Not measured: real terminal output time (depends on the emulator), memory
of the interactive process, Linux, and Windows.

## Phase 4: synchronization — 2026-11-06

Machine: macOS (Darwin 25.5.0, Apple M5), Git 2.54.0, release build, FSEvents.

```sh
cargo run --release --example bench_sync -- 5
```

- Repositories of 100, 5,000, and 50,000 tracked files are generated under
  `$TMPDIR/diffkit-bench-sync`.
- Each scenario uses a real `SyncService`: a real watcher, quiet period
  200 ms, maximum delay 1 s.
- Each scenario gets 1 warm-up run and 5 measured runs; medians are shown.
- Git processes are counted in-process: each `git` spawn increments a
  thread-local counter on the sync worker.
- **Latency** is measured from the end of the external change to the
  published state, and includes the deliberate 200 ms quiet period.
- **Events** are raw watcher callbacks. FSEvents reports several per
  write.

| Tracked | Scenario | Events | Reconciliations | Git processes | Reconcile time | Latency |
|---:|---|---:|---:|---:|---:|---:|
| 100 | one file saved | 4 | 1 | 5 | 42.7 ms | 258 ms |
| 100 | 10 events, one file | 8 | 1 | 5 | 22.3 ms | 238 ms |
| 100 | 100 events, 10 files | 424 | 1 | 5 | 45.9 ms | 250 ms |
| 100 | 1,000-event burst | 4124 | 1 | 5 | 60.2 ms | 269 ms |
| 100 | save every 50 ms for 3 s | 224 | 3 | 15 | 41.7 ms | 0 ms |
| 100 | edit + git add (index) | 18 | 1 | 7 | 48.0 ms | 265 ms |
| 100 | branch switch | 29 | 1 | 6 | 43.5 ms | 256 ms |
| 100 | manual refresh (full reconciliation) | 0 | 1 | 7 | — | 53 ms |
| 5000 | one file saved | 4 | 1 | 5 | 57.9 ms | 272 ms |
| 5000 | 10 events, one file | 8 | 1 | 5 | 58.7 ms | 271 ms |
| 5000 | 100 events, 10 files | 424 | 1 | 5 | 54.8 ms | 261 ms |
| 5000 | 1,000-event burst | 4172 | 1 | 5 | 66.6 ms | 273 ms |
| 5000 | save every 50 ms for 3 s | 224 | 3 | 15 | 59.1 ms | 0 ms |
| 5000 | edit + git add (index) | 18 | 1 | 7 | 63.9 ms | 279 ms |
| 5000 | branch switch | 29 | 1 | 6 | 66.0 ms | 278 ms |
| 5000 | manual refresh (full reconciliation) | 0 | 1 | 7 | — | 69 ms |
| 50000 | one file saved | 25 | 1 | 5 | 181.9 ms | 398 ms |
| 50000 | 10 events, one file | 8 | 1 | 5 | 181.4 ms | 394 ms |
| 50000 | 100 events, 10 files | 424 | 1 | 5 | 182.5 ms | 388 ms |
| 50000 | 1,000-event burst | 4128 | 1 | 5 | 186.7 ms | 394 ms |
| 50000 | save every 50 ms for 3 s | 224 | 3 | 15 | 182.1 ms | 0 ms |
| 50000 | edit + git add (index) | 20 | 1 | 7 | 186.5 ms | 425 ms |
| 50000 | branch switch | 29 | 1 | 6 | 184.5 ms | 396 ms |
| 50000 | manual refresh (full reconciliation) | 0 | 1 | 7 | — | 210 ms |

Reading the table:
- Every burst, from 8 to over 4,000 watcher events, collapses into one
  reconciliation of 5 Git processes. For working-tree triggers, only the
  Staged list is reused.
- "Save every 50 ms for 3 s": the quiet period never elapses, so the 1 s
  maximum delay forces a run about once a second (3 runs). Its latency
  column (0 ms) is not meaningful: the final state had already been
  published by a maximum-delay run before the measurement began. Treat it
  as not applicable.
- Index and branch changes rediscover all lists: 6–7 Git processes.
- Manual refresh skips the debounce. Its latency is the full
  reconciliation time, including ignored-directory detection.
- Reconciliation cost grows with repository size because `git status`
  and the list commands scan the index: about 43 ms at 100 files, 58 ms at
  5,000, and 182 ms at 50,000.

Cache (5,000-file repository, 200 changed files, interface state
in-process):

| Scenario | Result |
|---|---|
| Two files, back and forth, 100 moves | 1 Git call; 99 hits / 2 misses (the misses include the first file loaded before counting) |
| Walk 200 files, second pass, 64 MiB budget | 0 Git calls, 0.1 ms, 100% hits; 200 entries, 0.45 MiB |
| Same walk, 256 KiB budget | second pass 200 Git calls; 289 evictions; peak 256 KiB; resident 254 KiB |
| Reload of the shown file after it changed | 3.55 ms, 1 Git call, 1 invalidation |
| One 200,000-line diff (16,000 display rows) | 6.83 MiB charged to the cache |

Process memory for the whole benchmark run, from `/usr/bin/time -l`, is
not a measurement of the interactive viewer:
- maximum resident set size 65,339,392 bytes;
- peak memory footprint 33,063,368 bytes;
- 309.66 s real time.

Not measured:
- Linux (inotify) and Windows (ReadDirectoryChangesW);
- network filesystems;
- the memory of a long-running interactive session;
- CPU use while idle (the input thread wakes every 200 ms; the sync
  worker blocks).

## Phase 5: release measurements — 2026-11-07

Same machine: macOS 26.5 (arm64), Apple M5, Git 2.54.0, rustc 1.98.1. The
release profile now uses LTO and one codegen unit (`Cargo.toml`), so all
Phase 5 numbers are from that build.

### New scenarios

```sh
cargo run --release --example bench_release -- 5
```

In-process; 5 measured runs after 1 warm-up (medians); frames rendered into
a 200×60 `TestBackend`.

#### 1,000 changed files (5,000 tracked)

| Measure | Result |
|---|---:|
| App::open (snapshot + 4 lists) | 96.4 ms, 6 Git |
| First frame | 0.18 ms |
| First diff load | 3.84 ms, 1 Git |
| Manual refresh (inline, full) | 96.3 ms, 6 Git |
| Walk 1,000 files twice (first pass loads, second is cached) | 3606 ms |
| Cache, 64 MiB budget | second pass 100.0% hits; 1000 entries, 0 evictions, peak 3.35 MiB |
| Same walk, 256 KiB budget | 2000 misses, 2000 evictions |

#### Burst: 1,000 writes to 1,000 files, real watcher

Median: 2064 watcher events, 1 reconciliations, first result 341 ms after the last write.

#### Three linked worktrees (5,000 tracked files)

App::open of a linked worktree: 36.7 ms median over all three, 7 Git.
A commit published to `main` from worktree 2 appears as "behind" in worktree 1: 242 ms median after the ref moved.

#### History (1,001 commits)

| Measure | Result |
|---|---:|
| List 200 recent commits (L) | 4.4 ms, 1 Git |
| Open a commit (its file list) | 3.3 ms, 1 Git |
| Load its first file | 3.5 ms, 1 Git |

Notes:
- The 1,000-file walk loads every file on the first pass (about 3.5 ms
  each, one `git diff-files` per file). The second pass is answered
  entirely from the cache.
- With a 256 KiB budget, every move evicts and reloads.
- "Burst": 1,000 writes produced 2,064 watcher events and one
  reconciliation. The latency includes the 200 ms quiet period.
- "Linked worktrees": a commit made in worktree 2 and published to the
  shared `main` with `update-ref` appears in worktree 1 (whose upstream is
  `main`) as "behind" 242 ms later (median).
- Whole-process memory for `bench_release`, from `/usr/bin/time -l`:
  - maximum resident set size 19,709,952 bytes;
  - peak memory footprint 12,796,360 bytes;
  - 111.5 s real time.

### Phase 1 and Phase 3 benchmarks re-run unchanged

`bench_tui -- 7` (Phase 3 method):

| Tracked | Phase 3 `App::open` | Phase 5 `App::open` | Phase 3 first diff | Phase 5 first diff |
|---:|---:|---:|---:|---:|
| 100 | 26.66 ms | 19.70 ms | 3.92 ms | 3.25 ms |
| 5,000 | 40.47 ms | 40.25 ms | 3.75 ms | 3.77 ms |
| 50,000 | 306.81 ms | 305.87 ms | 5.84 ms | 6.07 ms |

200,000-line file:
- `load_file_diff`: 573.47 → 585.93 ms;
- display rows: 7.94 → 8.81 ms;
- one frame: 0.207 → 0.208 ms;
- scroll + frame: 0.222 → 0.222 ms.

The same 6 Git processes run at startup and 1 per diff, so operation
detection and conflict stages added no Git process. Process memory for
`bench_tui`: maximum RSS 67,747,840 bytes, peak footprint 31,457,712 bytes.

`bench_inspect -- 15` (Phase 1 method, end-to-end `diffkit inspect`):

| Repo | Scenario | Phase 1 median | Phase 5 median | `git status` then → now |
|---|---|---:|---:|---:|
| small | clean | 10.29 ms | 11.38 ms | 3.52 → 3.62 ms |
| small | dirty | 10.32 ms | 11.55 ms | 3.56 → 3.76 ms |
| medium | clean | 15.59 ms | 17.05 ms | 8.74 → 9.14 ms |
| medium | dirty | 15.90 ms | 17.28 ms | 9.05 → 9.29 ms |
| large | clean | 63.11 ms | 98.54 ms | 56.17 → 81.44 ms |
| large | dirty | 66.06 ms | 67.66 ms | 58.37 → 58.99 ms |

Reading the comparison:
- **Large clean (+35 ms):** environmental. Plain `git status` slowed by
  25 ms in the same run, and its individual runs ranged 93–126 ms (Phase
  1: 62–64 ms).
- **Small and medium (+1.1 to +1.5 ms):** not isolated.
  - Git calls are unchanged (3).
  - An interleaved A/B on the small repository, with the same code built
    with and without LTO, gave 11.91 ms and 12.02 ms, with `git status` at
    4.25 ms in that session (3.52 ms in Phase 1). So the release profile
    is not the cause, and the machine was slower than during Phase 1.
  - The only new work in `inspect` is about 10 `lstat` calls for
    operation detection.
  - It is reported as an unexplained difference of about 1 ms, not as a
    measured regression.

Not measured: Linux, Windows, or macOS Intel; a real terminal emulator's
output time; idle CPU; long interactive sessions.

## Optimization pass — 2026-11-08

Same-session comparison of the committed Phase 5 code (`f06eae4`, exported
with `git archive`) and the optimized working tree. Both were built with
the same release settings and a shared benchmark program, and run
alternately against the same scratch repositories on this Mac (Apple
Silicon, Git 2.54.0). Medians of 9 runs (5 for alignment) after 2
warm-ups; two rounds gave the same numbers within 1 ms.

Repository: 20,000 tracked files, 50 modified, 30 untracked. "Stat-dirty":
additionally 11,000 files whose modification time changed but whose content
did not, which Git re-checks on every read until the index is refreshed,
which Diffkit never does.

| Scenario | Before | After |
|---|---:|---:|
| Full refresh, refreshed index | 80.1 ms, 6 Git | 54.8 ms, 5 Git |
| Worktree-triggered refresh, refreshed index | 75.9 ms, 5 Git | 44.3 ms, 3 Git |
| Full refresh, stat-dirty | 691.8 ms, 6 Git | 164.1 ms, 5 Git |
| Worktree-triggered refresh, stat-dirty | 690.4 ms, 5 Git | 154.0 ms, 3 Git |
| Align 20,000 replaced short lines | 48.5 ms | 10.6 ms |
| Align 20,000 replaced 300-byte lines | 1,157.0 ms | 552.6 ms |
| `diff-stats --load`, 1,000 files × 400 changed lines: wall | 2.98 s | 1.11 s |
| same: peak footprint / max RSS | 157.9 / 199.9 MiB | 111.0 / 116.0 MiB |

`diff-stats --load` printed byte-identical output before and after. Where
the time went before, per command on the same repository: `status` 25 ms
(135 ms stat-dirty), `diff-files` 19 ms (268 ms), `diff-index HEAD` 19 ms
(298 ms), `ls-files --others` 11 ms. Limited to the 50 paths status
reported, `diff-files` took 9.8 ms and `diff-index HEAD` 10.1 ms in the
stat-dirty case.

Diff loads now run on a loader thread, so the interface keeps responding
during a slow load (verified by the PTY tests and the stale-result tests;
input latency during a load was not measured).

Not measured: Linux, Windows, or macOS Intel; the existing `bench_*`
examples were not rerun for this pass.
