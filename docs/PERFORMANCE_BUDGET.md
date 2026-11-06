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
