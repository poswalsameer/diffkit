# Phase 1 — Git Intelligence Engine

## Scope

A reliable Rust foundation that inspects a local Git repository and
accurately understands its current state, exposed through
`diffkit inspect`. Phase 1 does not render diffs, provide a terminal UI,
or watch the filesystem.

## Implementation requirements

1. **Command layer** — all Git calls go through `git::commands` using
   `std::process::Command`, discrete arguments, `-C <dir>`, no shell,
   `GIT_OPTIONAL_LOCKS=0`, and `GIT_NO_LAZY_FETCH=1`. Missing or
   non-executable Git produces a meaningful error.
2. **Repository discovery** — root, actual Git directory, common Git
   directory, current directory, inside-work-tree, worktree kind,
   shallowness. Works from nested directories, in linked worktrees (where
   `.git` is a file), and with paths containing spaces or Unicode.
3. **Status engine** — `git status --porcelain=v2 -z --branch
   --ahead-behind --untracked-files=all`, parsed as bytes. Supports type 1,
   type 2 (rename and copy, with the NUL-separated original path), `u`, `?`,
   `!`, and `#` records. Keeps the index and worktree dimensions separate,
   preserves raw XY, conflict kind, similarity, file kind, and submodule
   state. Paths are never lossily converted.
4. **Branch intelligence** — current branch, HEAD commit, detached HEAD,
   unborn branch, empty repository, unknown HEAD; resolved through Git.
5. **Upstream intelligence** — not configured, not applicable (detached),
   configured but missing locally, or available with its commit. Missing
   upstreams are states, not crashes. Nothing is configured, created, or
   fetched.
6. **Commit tracking** — ahead/behind by commit-graph reachability against
   the locally known upstream. Unavailable counts are never shown as zero.
7. **Snapshot** — one typed `RepositorySnapshot` combining all layers, with
   a single status call.
8. **CLI** — `diffkit`, `diffkit inspect`, `diffkit --help`,
   `diffkit --version`. The report shows repository paths and type; branch,
   HEAD, and state; upstream, its commit, tracking counts, and a staleness
   notice; staged, unstaged, partially staged, untracked, conflicted,
   deleted, renamed, and copied counts and file lists; and a clean state.
   Expected failures produce useful messages on stderr.
9. **Read-only and offline** — no write and no network operation; verified
   by tests.
10. **Tests, documentation, performance measurement.**

## Acceptance criteria

Repository discovery: root; Git directory; common Git directory; nested
directories; standard repositories; worktrees; detached HEAD; empty
repositories; non-repository directories.

Status engine: porcelain v2 used; NUL-separated output parsed correctly;
staged, unstaged, partially staged, untracked, deleted, and renamed files
identified; copies handled when reported; conflicts identified; unusual
filenames preserved; ignored files not treated as untracked.

Branch and upstream: current branch; HEAD commit; detached HEAD represented
correctly; upstream identified when configured; missing upstream handled;
missing tracking ref handled.

Commit tracking: ahead and behind counts correct; divergence represented;
no-upstream state represented; local tracking information not presented as
live remote state; no automatic fetch.

CLI: `--help`, `--version`, `inspect` work; output readable and reflects
real state; expected failures produce useful messages.

Engineering: no TUI; no diff rendering; no write operations; no network
operations; no unapproved dependencies; Git commands executed safely;
modular code; documentation present; comprehensive tests; regression tests
pass; performance measured; no known critical correctness bugs.

The status of each criterion, with evidence, is recorded in
`docs/PHASE_1_IMPLEMENTATION_REPORT.md`.
