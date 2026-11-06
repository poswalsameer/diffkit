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
