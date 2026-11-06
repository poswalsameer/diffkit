# DIFFKIT — PHASE 1 IMPLEMENTATION REPORT

Date: 2026-11-06. Host: macOS 26.5 (arm64), Apple M5, 16 GiB RAM, Git 2.54.0,
rustc/cargo 1.98.1.

## 1. Executive Summary

**Status: Phase 1 is complete on the tested platform (macOS/Unix). Windows
behavior is implemented and compiles, but has not been executed.**

This pass:

- Wired `RepositorySnapshot` into the application flow. `git::snapshot::inspect`
  builds one coherent snapshot from repository, branch, upstream, tracking,
  and status data. It uses **one** `git status` call, whose headers feed the
  branch and commit layers; the old code ran `git status` twice. A typical
  inspection with an upstream costs 3 Git subprocesses, measured with a
  logging wrapper.
- Rebuilt the CLI report around the snapshot. It shows repository root,
  current directory, Git and common directories, and worktree type; branch,
  HEAD, and branch state; upstream, its commit, ahead/behind, and a
  staleness notice; counts for staged, unstaged, partially staged,
  untracked, conflicted, deleted, renamed, and copied files; per-section
  file lists; and an explicit clean state.
- Fixed correctness defects found during review and testing:
  - Conflict XY codes were discarded.
  - `(unknown)` HEAD was mapped to "unborn".
  - Repository paths were `trim()`med, losing trailing spaces.
  - Non-UTF-8 repository paths failed.
  - `diffkit inspect | head` could panic on a closed pipe.
  - Windows paths were decoded lossily.
  - Windows `canonicalize` produced `\\?\` paths that Git never prints.
  - A missing upstream was not separated from an upstream that exists.
  - Partial clones could lazily fetch from the network during `git status`.
- Expanded tests from 6 integration tests and 0 unit tests to **36 unit and
  97 integration tests, all passing**. Every XY state Git produced in the
  probes, all seven conflict codes, local bare-remote tracking scenarios,
  read-only fingerprints with positive controls, and a Git invocation audit
  are covered.
- Re-ran the benchmarks with warm-up and repeated measured runs, clean and
  dirty, with medians and individual values, and explained the earlier
  447 ms first run.

All five quality gates pass with no warnings. No dependencies were added.
Phase 2 has not been started.

## 2. Final Repository Structure

```text
Cargo.toml, Cargo.lock, .gitignore, README.md
src/
  main.rs                 entry: argv → app::cli::run → exit code
  lib.rs                  pub mod git
  app/mod.rs
  app/cli.rs              argument handling, dispatch, exit codes, broken-pipe handling
  app/report.rs           snapshot → text report; display-only path escaping
  git/mod.rs
  git/commands.rs         single process-spawn site; path-byte conversion
  git/errors.rs           GitError
  git/models.rs           all typed models
  git/repository.rs       discovery
  git/status.rs           porcelain v2 reader and byte parser
  git/branches.rs         branch / HEAD / upstream
  git/commits.rs          ahead/behind
  git/snapshot.rs         RepositorySnapshot assembly
tests/
  common/mod.rs           TempDir, Repo, Remote fixtures, binary runner
  repository_discovery.rs  git_status.rs  branch_detection.rs  upstream_detection.rs
  commit_tracking.rs  repository_states.rs  unusual_filenames.rs  error_handling.rs
  cli_inspect.rs  read_only.rs
examples/bench_inspect.rs reproducible benchmark harness
docs/  PROJECT_OVERVIEW.md ARCHITECTURE.md TECH_STACK.md ENGINEERING_RULES.md
       DATA_MODEL.md TESTING_STRATEGY.md PERFORMANCE_BUDGET.md
       PHASE_1_IMPLEMENTATION_REPORT.md
docs/phases/  PHASE_1_GIT_ENGINE.md PHASE_2_DIFF_ENGINE.md PHASE_3_TERMINAL_UI.md
              PHASE_4_SYNCHRONIZATION_PERFORMANCE.md PHASE_5_ADVANCED_SUPPORT_RELEASE.md
```

About 5,400 lines of Rust in total, including tests and the benchmark.
Module responsibilities are described in `docs/ARCHITECTURE.md`.

## 3. Implemented Features

| Feature | How it works | Source | Tests |
|---|---|---|---|
| Safe Git execution | `Command::new("git")`, `-C <dir>`, discrete `OsStr` args, stdin closed, `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_LAZY_FETCH=1`. Spawn failures map to `GitNotFound`, `GitNotExecutable`, or `Io`; non-zero exits to `CommandFailed{args, code, stderr}`. | `git/commands.rs` | `read_only::only_the_command_layer_starts_processes`, `read_only::audit::*`, `error_handling::git_executable_*`, `commands::tests::*` |
| Repository discovery | One `rev-parse --path-format=absolute --is-inside-work-tree --is-bare-repository --is-shallow-repository --absolute-git-dir --git-common-dir --show-toplevel`. A first-line `false` on failure means not a work tree; "not a git repository" in stderr means not a repository; anything else is a `CommandFailed`. If a path contains `\n` (line count ≠ 6), it falls back to one query per path. | `git/repository.rs` | `repository_discovery::*` |
| Worktrees | `LinkedWorktree` when `git_dir ≠ common_git_dir`; `.git` as a file is handled by Git. | `git/repository.rs` | `linked_worktree_has_own_git_dir_and_shared_common_dir`, `multiple_worktrees_are_inspected_independently` |
| Status parsing | NUL-terminated byte records; types `1`, `2` (consumes the extra original-path field), `u`, `?`, `!`, `#`. Keeps raw XY, both dimensions, conflict kind, similarity, file kind from modes, and submodule state. Unknown letters become `Unknown(b)`; malformed records are errors. | `git/status.rs` | `status::tests::*`, `git_status::*`, `unusual_filenames::*` |
| Branch / HEAD | From `# branch.oid` and `# branch.head`: `Branch`, `Detached`, `Unborn`, `Unknown`. `for-each-ref --count=1` distinguishes an empty repository from an orphan branch, only when unborn. | `git/branches.rs` | `branch_detection::*`, `branches::tests::*` |
| Upstream | No header gives `NotConfigured` (or `NotApplicable` when detached). Header plus `branch.ab` runs `rev-parse @{upstream} --symbolic-full-name @{upstream}` and gives `Available`. Header without `branch.ab` on a born branch gives `Missing`. Unborn uses `rev-parse --verify --quiet @{upstream}`, where exit 1 means `Missing`. | `git/branches.rs` | `upstream_detection::*` |
| Commit tracking | `# branch.ab +A -B`, which Git computes by commit-graph reachability, the same as `rev-list --left-right --count`. Unavailable states carry a reason. | `git/commits.rs` | `commit_tracking::*` (every count cross-checked with `rev-list`), `commits::tests::*` |
| Snapshot | Discovery → one status call → branch → upstream → tracking. | `git/snapshot.rs` | `repository_states::snapshot_combines_every_layer_coherently`, all integration tests via `Repo::snapshot` |
| CLI | `diffkit` / `--help` / `-h` / `help` print usage (exit 0); `--version` / `-V`; `inspect`; unknown or extra arguments exit 2; inspection errors print `diffkit: error: …` and exit 1; EPIPE on stdout exits 0. | `app/cli.rs`, `app/report.rs` | `cli_inspect::*`, `report::tests::*` |
| Path display | Paths are quoted and C-escaped only when ambiguous: control characters, quotes, backslash, invalid UTF-8 (`\xff`), surrounding whitespace, ` -> `. | `app/report.rs` | `report::tests::*`, `unusual_filenames::unusual_names_are_escaped_in_cli_output` |

## 4. Git Command Inventory

Every command is run as `git -C <dir> …` with `GIT_OPTIONAL_LOCKS=0` and
`GIT_NO_LAZY_FETCH=1`, stdin closed. All are read-only and local.

| # | Command | When | Purpose / expected output | Error handling |
|---|---|---|---|---|
| 1 | `rev-parse --path-format=absolute --is-inside-work-tree --is-bare-repository --is-shallow-repository --absolute-git-dir --git-common-dir --show-toplevel` | Always | 3 booleans + 3 absolute paths, one per line | Exit ≠ 0 and first line `false` gives `NotAWorkTree`; stderr containing "not a git repository" gives `NotARepository`; otherwise `CommandFailed`. Non-boolean lines give `MalformedOutput`. |
| 1a | `rev-parse --path-format=absolute <one of the three path flags>` | Only if a repository path contains `\n` | One path | `CommandFailed` |
| 2 | `status --porcelain=v2 -z --branch --ahead-behind --untracked-files=all` | Always | NUL-terminated headers and entries | `CommandFailed` (e.g. corrupt index); `MalformedOutput` on parse errors |
| 3 | `rev-parse @{upstream} --symbolic-full-name @{upstream}` | Born branch whose status shows `branch.upstream` and `branch.ab` | Upstream OID, full ref name | `CommandFailed`; OID validated |
| 3u | `rev-parse --verify --quiet @{upstream}` | Unborn branch with `branch.upstream` | OID (exit 0) or nothing (exit 1 = missing) | Other exits give `CommandFailed` |
| 4 | `for-each-ref --count=1 --format=%(refname)` | Unborn HEAD only | Empty output means empty repository | `CommandFailed` |

Subprocesses per inspection, measured by `read_only::audit`: upstream
available 3; no upstream 2; missing upstream 2; detached 2; empty
repository 3; unborn with upstream 4; not a repository 1.

## 5. Data Model Explanation

See `docs/DATA_MODEL.md` for full definitions. In summary:

- `RepositorySnapshot { repository, branch, upstream, commit_tracking, status }`.
- `FileStatus` keeps `index` and `worktree` as independent `Change` values
  plus raw `xy`.
  - Staged means `index ≠ Unmodified`; unstaged means `worktree ≠ Unmodified`.
  - Partially staged means both.
  - Untracked is a `??` record, where both sides are `Untracked`.
  - Conflicted is a `u` record: both sides `Unmerged`, plus a `ConflictKind`
    for `DD AU UD UA DU AA UU`.
  - Conflicts and untracked entries are never counted as staged or unstaged.
- `UpstreamInfo` is `NotConfigured | NotApplicable | Missing{name} |
  Available{name, full_ref, commit}`.
- `CommitTrackingInfo` is `Compared{ahead, behind} | Unavailable(reason)`.
  `ahead()` and `behind()` return `None` when unavailable, never `0`.
- Paths are `PathBuf`, byte-exact on Unix and strict UTF-8 on Windows.

## 6. Unit Test Results

36 unit tests: 31 in the library, 5 in the binary. 36 passed, 0 failed, 0
ignored.

- `git::status` (12): `branch_headers`, `copy_record`, `deleted_file_kind_falls_back_to_head_mode`, `empty_output_is_clean`, `malformed_inputs_are_errors_not_panics` (12 malformed inputs), `non_utf8_path_bytes_are_preserved`, `ordinary_record_preserves_both_dimensions_and_spaces`, `rename_record_consumes_original_path_field`, `submodule_field`, `unknown_status_letter_is_preserved_not_rejected`, `unmerged_record_keeps_conflict_kind`, `untracked_and_ignored_are_kept_apart`
- `git::models` (6): `staged_and_unstaged_are_independent_dimensions` (15 XY codes), `conflicts_and_untracked_are_neither_staged_nor_unstaged`, `status_counts`, `tracking_relation_and_unavailable_counts`, `object_id_validation`, `conflict_codes_and_modes`
- `git::branches` (5), `git::commits` (2), `git::commands` (3, plus 1 `#[cfg(windows)]` test not run on this host), `git::errors` (3)
- `app::report` (5): path quoting/escaping, non-UTF-8 display, partially staged rendering, staleness disclaimer, no invented zero counts

## 7. Integration Test Results

97 integration tests: 97 passed, 0 failed, 0 ignored. Two tests contain
runtime skip branches:

- `unusual_filenames::non_utf8_filename_on_disk_where_filesystem_allows`
  **skipped its assertions on this host**. APFS rejects non-UTF-8 names
  (`os error 92`), and the test prints `SKIPPED`. Coverage of raw non-UTF-8
  bytes in real Git output comes from `non_utf8_filename_from_real_git_index`,
  which passed.
- `error_handling::inaccessible_paths` skips only when run as root. It ran
  fully here.

| File | Tests | Highlights |
|---|---:|---|
| `repository_discovery.rs` | 10 | root, 3 nested levels, `.git` dir vs file, linked worktree, 3 worktrees (one detached) with independent status, spaces + Unicode + emoji in repo path, shallow clone, non-repo, bare repo / `.git` dir, missing path |
| `git_status.rs` | 23 | `.M M. MM MD A. AM AD .A D. .D R. RM RD .R C. .T T.`, executable bit, multiple/nested untracked, ignored, tracked-but-ignored, submodule `SC.U`, combined matrix counts, exact status args |
| `branch_detection.rs` | 7 | normal + HEAD hash (vs `rev-parse`), detached at `HEAD~1`, empty, unborn `trunk/dev`, orphan (not empty), 5 unusual valid names, external switches |
| `upstream_detection.rs` | 9 | available (name / full ref / commit), slash branch, no upstream, no remote, never-fetched, deleted + pruned, local-branch upstream, detached, unborn with existing and missing upstream |
| `commit_tracking.rs` | 12 | (0,0); (1,0), (3,0); (0,1), (0,4); (2,3); merge → (2,0); back-dated commit; no upstream / missing / deleted / detached unavailable; stale ref (behind 0 before fetch, 2 after, ref unchanged by inspect); shallow clone |
| `repository_states.rs` | 6 | UU (and resolution → `M.`); UU+UD+DU+AA together; DD+AU+UA from rename/rename; rebase conflict (detached); full snapshot coherence; empty repository snapshot |
| `unusual_filenames.rs` | 5 | 15 hostile names through untracked → staged → modified → renamed (both paths); non-UTF-8 via index (`AD`, then `.D`); on-disk non-UTF-8 (skipped on APFS); CLI escaping; portable Unicode subset |
| `error_handling.rs` | 10 | not a repo, broken `.git` file, corrupt index (`CommandFailed`, code 128, stderr), corrupt HEAD, Git missing from PATH, Git not executable, permission-denied directory, nonexistent dir, malformed output, expected states exit 0 |
| `cli_inspect.rs` | 11 | help/version/default, usage errors (exit 2), exact field output for clean, dirty (diverged 2/1), conflicts, detached, empty, missing upstream, no upstream, nested + linked worktree, closed stdout |
| `read_only.rs` | 4 | fingerprint unchanged plus positive control; partial clone never fetches plus positive control; Git invocation audit; spawn-site source audit |

## 8. Build and Validation Results

Run after the final code change, in this order, on 2026-11-06:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0, no diff |
| `cargo check` | exit 0, no warnings |
| `cargo build --release` | exit 0, no warnings |
| `cargo test` | exit 0 — 31 + 5 unit, 97 integration, 0 doc-tests; 133 passed, 0 failed, 0 ignored |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0, no warnings |
| `cargo clippy --target x86_64-pc-windows-msvc --all-targets --all-features -- -D warnings` (extra) | exit 0, no warnings. Compile and lint only; nothing was executed on Windows. The `x86_64-pc-windows-msvc` std component was added to the local rustup toolchain for this; remove it with `rustup target remove x86_64-pc-windows-msvc`. |

Warnings found during the work were fixed, not suppressed:
- A needless explicit lifetime in a test helper.
- An overly complex tuple type in `report.rs`, given a named `Group` alias.
- Unix-only imports that warned on Windows, now `#[cfg(unix)]`.

The only `allow` attribute is `#![allow(dead_code)]` in `tests/common/mod.rs`.
That is the standard idiom for a helper module compiled separately into each
integration-test crate, each using a different subset. It is not used in
application code.

## 9. Performance Measurements

`cargo run --release --example bench_inspect -- 15`: 3 warm-up and 15
measured runs per scenario, release binary, end to end with output to
`/dev/null`. Each repository tracks a local bare remote and is one commit
ahead. Full method, individual runs, and the first-pass data are in
`docs/PERFORMANCE_BUDGET.md`.

| Repo | Tracked | Scenario | Changed entries | Git calls | diffkit median | git status median | diffkit own peak footprint | Max RSS in process tree |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| small | 100 | clean | 0 | 3 | 10.29 ms | 3.52 ms | 1.1 MiB | 8.2 MiB |
| small | 100 | dirty | 16 | 3 | 10.32 ms | 3.56 ms | 1.1 MiB | 8.2 MiB |
| medium | 5,000 | clean | 0 | 3 | 15.59 ms | 8.74 ms | 1.1 MiB | 9.5 MiB |
| medium | 5,000 | dirty | 160 | 3 | 15.90 ms | 9.05 ms | 1.2 MiB | 9.5 MiB |
| large | 50,000 | clean | 0 | 3 | 63.11 ms | 56.17 ms | 1.0 MiB | 18.4 MiB |
| large | 50,000 | dirty | 1,600 | 3 | 66.06 ms | 58.37 ms | 1.7 MiB | 18.4 MiB |

Dirty sets: small has 5 staged, 9 unstaged, 3 partially staged, 5
untracked, and 1 deleted; medium has 50/85/25/50/10; large has
500/850/250/500/100. Ranges across the measured runs: small 10.06–10.98 ms;
medium 15.21–16.98 ms; large 62.12–66.98 ms.

Every scenario is far inside its target (300 ms / 1 s / 3 s) **on this
machine**. Diffkit adds about 7 ms over raw `git status`. "Max RSS in
process tree" is Git's; Diffkit's own process stays under 2 MiB.

**The earlier 447 ms first run.** The harness copies the release binary to
a new file and times `--version`, which runs no Git. The first launch of a
newly written executable took 169.96, 125.82, and 129.16 ms in three
trials; the second launch took 3.14, 4.15, and 3.66 ms. On this macOS host
the first execution of a new binary has a one-time cost of roughly
125–170 ms, consistent with first-run executable verification. Together
with cold caches, that explains the outlier; it is not inspection cost.

**First-pass variance.** A pass run immediately after writing the
50,000-file fixture measured large/clean at 92.97 ms (88.9–94.6 ms), with
raw `git status` also elevated at 71.9–85.0 ms. The rerun on the same
repositories was stable at 62–64 ms. The source of the disturbance was not
instrumented.

**Not measured:** Linux, Windows, cold filesystem caches (purging needs
root), fsmonitor or untracked-cache configurations, and submodule-heavy
repositories.

## 10. Read-Only Verification

1. **Fingerprints**, in `read_only::inspect_leaves_every_repository_file_untouched`.
   - The fixture contains a work repository with staged, partially staged,
     untracked, and stat-dirty files (mtime changed, content unchanged), one
     commit ahead of a local bare remote. It also has a linked worktree with
     changes, a repository mid-merge with a conflict, and an unborn
     repository with a staged file.
   - Before and after 15 `diffkit inspect` runs (3 rounds over 5
     directories), every file under the fixture is compared. That includes
     all `.git` contents, the bare remote, and worktree admin directories,
     by content bytes, permission mode, and mtime, plus symlink targets and
     the set of paths. Result: identical.
   - **Positive control:** plain `git status` without `GIT_OPTIONAL_LOCKS`
     run on the same repository *does* rewrite `.git/index`, and the test
     asserts that it does. So the fingerprint can detect the write Diffkit
     avoids.
2. **Network**, in `read_only::partial_clone_inspection_never_fetches_missing_objects`.
   - In a `--filter=blob:none` partial clone, an inexact staged rename
     forces Git to read a blob that exists only on the promisor remote.
     `diffkit inspect` leaves `git count-objects -v` unchanged and exits 1
     with Git's "could not fetch … from promisor remote" error.
   - **Positive control:** the same `git status` without
     `GIT_NO_LAZY_FETCH` does fetch, and the object count changes.
3. **Command audit**, in `read_only::audit::every_git_invocation_is_an_allowed_read_only_command`.
   - A logging `git` wrapper is the only `git` on `PATH`. It records
     argv, `GIT_OPTIONAL_LOCKS`, and `GIT_NO_LAZY_FETCH`.
   - It covers seven scenarios: upstream available, no upstream, empty
     repository, not a repository, missing upstream, detached, and unborn
     with upstream.
   - Every invocation starts with `-C <dir>`, has `LOCKS=0 LAZY=1`, uses
     only `rev-parse`, `status`, or `for-each-ref`, and the status
     arguments equal `STATUS_ARGS` exactly.
   - None of add, commit, push, pull, fetch, reset, checkout, switch,
     merge, rebase, restore, or any other subcommand occurs.
4. **Stale-ref check**, in `commit_tracking::stale_tracking_ref_is_reported_as_locally_known_and_never_fetched`:
   `refs/remotes/origin/main` is unchanged after inspection while the
   remote has moved on.
5. **Source audit**, in `read_only::only_the_command_layer_starts_processes`:
   `Command::new(` appears exactly once, in `git/commands.rs`, as
   `Command::new(GIT)` with `GIT = "git"`. No `sh`, `bash`, `cmd`, or
   `powershell` appears anywhere in `src/`.

**Limitations.** Verification covers Git 2.54 on macOS.
- Git processes started *by Git itself* (submodule recursion inside
  `git status`) are not seen by the wrapper; they inherit both variables.
- A user-configured `core.fsmonitor` hook script would be executed by
  `git status`, as it would for the user's own `git status`.
- On Git < 2.45, `GIT_NO_LAZY_FETCH` is ignored.

## 11. Edge-Case Coverage

**Tested (passing):**
- Discovery: nested directories (3 levels); spaces, Unicode, and emoji in
  the repository path; `.git` as a file; multiple worktrees; shallow
  clones; bare repositories and `.git` directories; broken `.git` pointers.
- Status: all XY states listed in §7; all 7 conflict codes; copies (with
  `status.renames=copies`) and their absence without it; type changes;
  executable bit; submodule fields; ignored and tracked-but-ignored files.
- Filenames: tab, newline, CR, leading and trailing spaces, quotes,
  backslash, leading dash, ` -> `, glob characters, NFC, CJK, and emoji in
  rename records; raw non-UTF-8 bytes from real Git output.
- Branches: slash branch names, `@`, `+`, `#`, and Unicode branch names.
- Upstream: local-branch upstream; unborn branch with upstream; deleted and
  pruned upstream.
- Tracking: counts across a merge commit; back-dated commits; shallow
  tracking.
- Failures: corrupt index or HEAD; Git missing or not executable;
  permission-denied directories; a closed stdout pipe.

**Untested or partially tested:**
- Any execution on Windows (compiled and linted only).
- Non-UTF-8 filenames *on disk* on macOS, which the filesystem forbids.
- Rebase/cherry-pick details beyond conflicts and detached HEAD.
- `core.fsmonitor` and untracked-cache configurations.
- Sparse checkout.
- SHA-256 repositories (the parser accepts 64-hex IDs but no fixture uses
  them).
- Git versions other than 2.54.
- Non-English Git locales (affects only error classification).
- Concurrent modification during inspection.

## 12. Dependencies

None. `Cargo.toml` has no `[dependencies]` or `[dev-dependencies]`;
`Cargo.lock` lists only `diffkit`. Runtime requires the `git` executable
(2.31+; 2.45+ for the lazy-fetch guard). No dependency was added beyond the
approved choices.

## 13. Known Limitations

1. **Windows is not validated.** The strategy is implemented:
   - Git for Windows emits UTF-8 paths, which are decoded strictly; invalid
     bytes become an error, never a lossy substitution.
   - The start directory uses `std::path::absolute` instead of
     `canonicalize`, avoiding `\\?\` prefixes.
   It compiles and passes clippy for `x86_64-pc-windows-msvc`, but no test
   ran on Windows. Some tests are `#[cfg(unix)]` (symlinks, permissions,
   non-UTF-8 names).
2. **Ref names are stored as `String`.** Git allows non-UTF-8 bytes in ref
   names; such branch or upstream names would show replacement characters.
   File paths are lossless.
3. **`core.fsmonitor` hooks.** If configured, `git status` runs the hook;
   Diffkit does not override user configuration. Needs a decision (see §14).
4. **Partial clones.** With the lazy-fetch guard, inspection fails with
   Git's error when status needs a blob that is only on the promisor
   remote, instead of fetching it. On Git < 2.45 the guard is inactive.
5. **The snapshot is not atomic.** Up to four sequential Git commands run;
   a concurrent Git operation can interleave.
6. **Error classification is partly language-dependent.** Only the
   "not a git repository" case depends on English stderr. In other locales
   it becomes a generic `CommandFailed` that still carries Git's message.
7. **Git 2.31+ required.** Older Git yields a parse error, not a friendly
   version message.
8. **Shallow clones.** Counts are limited to local history; the report says
   so.
9. **Copies** are reported only when the user's Git configuration enables
   copy detection, which is Git's own behavior.
10. **Ignored files** are not listed (by design); `RepositoryStatus::ignored`
    stays empty in practice.
11. **No index refresh.** Because Diffkit never writes the index, stat-dirty
    entries are re-checked on every inspection.
12. **Recent upstream commit summaries** (optional in the specification)
    were not implemented.

## 14. Deviations from Specification

1. **Ahead/behind source.** Counts come from `git status`'s `# branch.ab`
   header rather than a separate `git rev-list --left-right --count`.
   The specification lists `branch.ab` and asks for reuse of
   multi-field commands. It is the same reachability computation, and
   every count is cross-checked against `rev-list` in tests.
2. **Model shapes.**
   - `UpstreamInfo` and `CommitTrackingInfo` are enums with accessor
     methods (`configured()`, `available()`, `comparison_available()`,
     `ahead()`, `behind()`) instead of structs with boolean fields.
   - `RepositoryStatus` counts are methods; `repository_type` is named
     `kind`; the branch name is reached through `HeadState` / `name()`.
   All specified information is retained.
3. **Additional modules and files** beyond the starting tree:
   `git/snapshot.rs`, `app/report.rs`, `tests/common/mod.rs`,
   `tests/cli_inspect.rs`, `tests/read_only.rs`, and
   `examples/bench_inspect.rs`.
4. **`GIT_NO_LAZY_FETCH=1`** is set on every Git call. This was not in the
   specification. It enforces the no-network rule, and it changes behavior
   in partial clones (error instead of fetch). **Flagged for architectural
   review.**
5. **Minimum Git version 2.31**, because of `rev-parse --path-format=absolute`.
6. **Report format** goes beyond the illustrative example: it adds the
   current directory, partially staged, deleted, renamed, and copied
   sections, per-line descriptions, and a "Local branch reference" source
   line for local upstreams.

## 15. Full Phase 1 Acceptance Checklist

| # | Criterion | Status | Evidence (tests / validation) |
|---|---|---|---|
| | **Repository discovery** | | |
| 1 | Repository root can be discovered | PASS | `repository_discovery::discovers_standard_repository_from_root`, `discovers_root_from_multiple_nested_levels`; `cli_inspect::clean_repository_with_upstream` |
| 2 | Git directory can be discovered | PASS | `discovers_standard_repository_from_root` (`git_dir == root/.git`), `linked_worktree_has_own_git_dir_and_shared_common_dir` (`.git/worktrees/linked`) |
| 3 | Common Git directory can be discovered | PASS | `linked_worktree_has_own_git_dir_and_shared_common_dir`, `multiple_worktrees_are_inspected_independently` |
| 4 | Nested directories work | PASS | `discovers_root_from_multiple_nested_levels`, `repository_path_with_spaces_and_unicode`, `cli_inspect::nested_directory_and_linked_worktree_output`, `repository_states::snapshot_combines_every_layer_coherently` |
| 5 | Standard repositories work | PASS | `discovers_standard_repository_from_root` (`RepositoryKind::Standard`, `.git` is a dir) |
| 6 | Worktrees are supported | PASS | `linked_worktree_has_own_git_dir_and_shared_common_dir`, `multiple_worktrees_are_inspected_independently` (3 worktrees, independent branch and status) |
| 7 | Detached HEAD is supported | PASS | `branch_detection::detached_head_is_explicit`, `multiple_worktrees_are_inspected_independently`, `cli_inspect::detached_head_output` |
| 8 | Empty repositories are handled | PASS | `discovers_empty_repository`, `branch_detection::empty_repository_has_unborn_branch`, `repository_states::empty_repository_snapshot`, `cli_inspect::empty_repository_output` |
| 9 | Non-repository directories are handled | PASS | `non_git_directory_is_reported`, `git_directory_and_bare_repository_are_not_work_trees`, `error_handling::not_a_git_repository`, `invalid_git_directory_pointer` |
| | **Status engine** | | |
| 10 | Porcelain v2 is used | PASS | `status::STATUS_ARGS`; `git_status::status_read_uses_one_porcelain_v2_call`; audit asserts the exact status argv |
| 11 | NUL-separated output is parsed correctly | PASS | `status::tests::rename_record_consumes_original_path_field`, `malformed_inputs_are_errors_not_panics`; `unusual_filenames::unusual_names_survive_every_record_type` (newline names in rename pairs) |
| 12 | Staged changes are identified | PASS | `git_status::staged_modification`, `staged_addition_and_follow_up_changes`, `staged_deletion`, `staged_rename_keeps_original_path`, `type_change_unstaged_and_staged`, `full_matrix_counts_in_one_repository` |
| 13 | Unstaged changes are identified | PASS | `unstaged_modification`, `unstaged_deletion`, `intent_to_add_is_an_unstaged_addition`, `worktree_rename_of_intent_to_add_file`, `executable_bit_change_is_a_modification` |
| 14 | Partially staged files are identified | PASS | `partially_staged_modification` (MM), `staged_modification_then_deleted` (MD), `staged_addition_and_follow_up_changes` (AM), `rename_then_modified_or_deleted` (RM); `cli_inspect::representative_dirty_state` |
| 15 | Untracked files are identified | PASS | `nested_untracked_files_are_listed_individually`, `multiple_modified_and_untracked_files` |
| 16 | Deleted files are identified | PASS | `staged_deletion`, `unstaged_deletion`, MD/AD/RD cases; `cli_inspect::representative_dirty_state` (DELETED section) |
| 17 | Renamed files are identified | PASS | `staged_rename_keeps_original_path`, `rename_then_modified_or_deleted`, `worktree_rename_of_intent_to_add_file` |
| 18 | Copied files are handled when reported | PASS | `copy_record_when_git_reports_copies` (real `C.` record via `status.renames=copies`), `copy_is_an_addition_without_copy_detection`, `status::tests::copy_record` |
| 19 | Conflicts are identified | PASS | `repository_states::single_merge_conflict`, `multiple_simultaneous_conflicts_of_different_kinds`, `rename_rename_conflict_reports_dd_au_ua`, `rebase_conflict_is_detached_with_conflicts`; `cli_inspect::conflicts_are_listed` |
| 20 | Unusual filenames are preserved | PASS (Unix host) | `unusual_names_survive_every_record_type`, `non_utf8_filename_from_real_git_index`, `unicode_and_space_names_portable_subset`, `unusual_names_are_escaped_in_cli_output`, `status::tests::non_utf8_path_bytes_are_preserved` |
| 20w | Native path handling on Windows | NOT TESTED | Implemented (`commands::path_from_git`, `repository::resolve_start`); compiles and passes clippy for `x86_64-pc-windows-msvc`; not executed |
| 21 | Ignored files are not treated as ordinary untracked files | PASS | `ignored_files_are_not_untracked`, `tracked_file_matching_ignore_rule_is_still_reported`, `status::tests::untracked_and_ignored_are_kept_apart` |
| | **Branch and upstream** | | |
| 22 | Current branch is identified | PASS | `normal_branch_and_correct_head_hash`, `branch_names_with_slashes_and_unusual_valid_characters`, `external_branch_switches_are_reflected`, `unborn_branch_with_custom_initial_name` |
| 23 | HEAD commit is identified | PASS | `normal_branch_and_correct_head_hash` (vs `git rev-parse HEAD`, before and after a new commit), `detached_head_is_explicit` |
| 24 | Detached HEAD is represented correctly | PASS | `detached_head_is_explicit` (`HeadState::Detached`, `name() == None`), `branches::tests::detached_head_is_not_a_branch_named_head`, `upstream_detection::detached_head_has_no_branch_upstream` |
| 25 | Upstream is identified when configured | PASS | `configured_upstream_with_existing_ref`, `branch_with_slashes_tracks_matching_remote_branch`, `upstream_can_be_a_local_branch`, `unborn_branch_with_configured_upstream` |
| 26 | Missing upstream is handled | PASS | `branch_without_upstream_while_remote_exists`, `repository_without_any_remote`, `cli_inspect::no_upstream_output`, `error_handling::expected_states_are_not_errors` |
| 27 | Missing tracking ref is handled | PASS | `configured_upstream_whose_ref_was_never_fetched`, `upstream_branch_deleted_on_remote_and_pruned`, `unborn_branch_with_configured_upstream`, `cli_inspect::missing_upstream_output` |
| | **Commit tracking** | | |
| 28 | Ahead count is correct | PASS | `commit_tracking::ahead_only_single_and_multiple` (1, 3), `merge_commit_counts_by_reachability` (2), `counts_ignore_commit_timestamps`; all cross-checked with `rev-list` |
| 29 | Behind count is correct | PASS | `behind_only_single_and_multiple` (1, 4), `stale_tracking_ref_is_reported_as_locally_known_and_never_fetched` (2 after fetch) |
| 30 | Divergence is represented correctly | PASS | `diverged_histories` (2 ahead / 3 behind, `Diverged`), `cli_inspect::representative_dirty_state` |
| 31 | No-upstream state is represented correctly | PASS | `no_upstream_is_unavailable_not_zero` (`ahead() == None`, not 0), `cli_inspect::no_upstream_output` (no "Ahead by" line) |
| 32 | Local tracking info not presented as live remote info | PASS | `stale_tracking_ref_is_reported_as_locally_known_and_never_fetched`; `cli_inspect::clean_repository_with_upstream` ("…not been verified by fetching; it may be stale."); `report::tests::tracking_counts_carry_the_staleness_disclaimer` |
| 33 | No automatic fetch occurs | PASS | audit (only rev-parse/status/for-each-ref), stale-ref test (ref unchanged), `partial_clone_inspection_never_fetches_missing_objects` (with positive control) |
| | **CLI** | | |
| 34 | `diffkit --help` works | PASS | `cli_inspect::help_version_and_default` |
| 35 | `diffkit --version` works | PASS | `cli_inspect::help_version_and_default` (exact output) |
| 36 | `diffkit inspect` works | PASS | all `cli_inspect` tests; `read_only` runs |
| 37 | Diagnostic output is readable | PASS | Labelled sections with exact-format assertions in `cli_inspect::*`; ambiguous paths escaped (`unusual_names_are_escaped_in_cli_output`). Readability is a judgment; a sample is in `cli_inspect::representative_dirty_state`. |
| 38 | Diagnostic output reflects real repository state | PASS | `cli_inspect::*` compare against Git's own values (`rev-parse HEAD`, `--absolute-git-dir`, real paths, counts) |
| 39 | Expected failures produce useful error messages | PASS | `error_handling::*` (not a repo, broken gitfile, corrupt index with stderr, Git missing, not executable, permissions, missing dir), `cli_inspect::usage_errors_exit_2` |
| | **Engineering** | | |
| 40 | No TUI is implemented | PASS | Source tree; no dependencies; `report.rs` is plain text |
| 41 | No diff rendering is implemented | PASS | Source tree; no diff commands in the audit allow-list |
| 42 | No write operations are implemented | PASS | `read_only::inspect_leaves_every_repository_file_untouched` (with positive control), audit |
| 43 | No network operations are performed | PASS (Git ≥ 2.45 for partial clones) | audit; `partial_clone_inspection_never_fetches_missing_objects` |
| 44 | No unapproved dependencies exist | PASS | `Cargo.toml` (no dependencies), `Cargo.lock` (only `diffkit`) |
| 45 | Git commands are executed safely | PASS | `read_only::only_the_command_layer_starts_processes`, audit (`-C` pinned, discrete args) |
| 46 | Code is modular | PASS | Layered modules per `docs/ARCHITECTURE.md`; CLI has no Git logic (it calls `snapshot::inspect_current_dir` only) |
| 47 | Documentation is present | PASS | All 7 docs, 5 phase docs, README, this report — updated to the implemented behavior |
| 48 | Tests are comprehensive | PASS (host scope) | 133 tests covering the specification's §19 matrix. Gaps listed in §11: Windows execution; on-disk non-UTF-8 on APFS |
| 49 | Regression tests pass | PASS | `cargo test`: 133 passed, 0 failed, after the final change |
| 50 | Performance is measured | PASS | §9; `docs/PERFORMANCE_BUDGET.md`; `examples/bench_inspect.rs` |
| 51 | No known critical correctness bugs remain | PASS | None known; non-critical limitations listed in §13 |

## 16. Final Recommendation

Phase 1 is **ready for architectural review** as a macOS/Unix-validated
implementation. Before that review signs off, please decide on:

1. Whether `GIT_NO_LAZY_FETCH=1` (fail rather than fetch in partial clones)
   is the intended behavior.
2. Whether `core.fsmonitor` hooks should be left to user configuration, as
   now, or disabled for inspection.
3. Whether Windows validation (running the suite on a Windows host) is
   required before Phase 1 is accepted, or belongs to Phase 5.

Phase 2 has not been started and will not begin without explicit approval.
