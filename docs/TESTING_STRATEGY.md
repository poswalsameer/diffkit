# Testing Strategy

Run everything with `cargo test`. Tests use only Rust's built-in harness.

## Layout

Every automated test lives in `tests/`; production files contain no
`#[cfg(test)]` code. Tests reach the library only through its public
interface, and the binary through `CARGO_BIN_EXE_diffkit`.

| Target | Covers |
|---|---|
| `tests/git.rs` + `tests/git/` | `diffkit::git`: discovery, status, branches, upstreams, ahead/behind, conflicts, renames, submodules, symlinks, operations, history, errors, read-only and offline guarantees |
| `tests/diff.rs` + `tests/diff/` | `diffkit::diff`: comparisons, lazy loading, paths, special files, untracked files, patch parsing, hunks, alignment |
| `tests/sync.rs` + `tests/sync/` | `diffkit::sync`: classification, debouncing, the worker, reconciliation, the cache, the real watcher, consistency under concurrent Git |
| `tests/tui.rs` + `tests/tui/` | `diffkit::tui`: application state, keys, layout, rendering, history screen, sync results, real pseudo-terminal sessions |
| `tests/cli.rs` + `tests/cli/` | `diffkit::cli` and the binary: argument parsing, `inspect`, `diff-stats` |
| `tests/types.rs` + `tests/types/` | `diffkit::types`: repository model predicates and labels |
| `tests/helpers.rs` + `tests/helpers/` | `diffkit::helpers`: display escaping and truncation, Git text conventions |
| `tests/panic_hook.rs` | The panic hook; its own target because panic hooks are process-wide |

Each target's root file declares `mod support;` (shared fixtures in
`tests/support/mod.rs`, the one `mod.rs`: a `tests/support.rs` would be
compiled as a test target of its own) and an inline `mod <feature> { … }`
listing the files in its folder. Run one area with `cargo test --test sync`,
or one file with a filter: `cargo test --test sync watch::`.

## Fixture tests (pure logic)

Pure logic with hand-written inputs, focused on what real Git output cannot
easily produce. These were unit tests inside `src/` until the
restructure; they now use only public functions, and the few that checked
private details were rewritten to check the same behavior through public
ones.

- `git/status_records.rs` — every record type, rename/copy original-path
  pairing, paths with spaces/tabs/newlines, raw non-UTF-8 bytes (Unix),
  submodule fields, unknown status letters, ignored records, and a table of
  malformed inputs that must return errors instead of panicking.
- `git/branch_headers.rs` — header interpretation for normal, detached,
  unborn, and unknown HEAD; malformed object IDs.
- `git/ahead_behind.rs` — `branch.ab` parsing (including `+? -?` and
  malformed values) and the mapping from upstream state to tracking
  availability.
- `types/repository.rs` — staged/unstaged/partial predicates over a table
  of XY codes, counts, tracking relation, object ID validation.
- `git/commands.rs`, `git/error_messages.rs`, `helpers/git_text.rs` —
  newline stripping that keeps other whitespace, path conversion per
  platform (Windows tests are `#[cfg(windows)]`), error messages.
- `helpers/display.rs` — path quoting and escaping, content sanitizing,
  width-aware truncation.
- `cli/inspect_report.rs` — rendering of partially staged files, untracked
  tracking, the staleness disclaimer, operations, and conflict stages.
- `cli/args.rs` — every accepted command line and the usage errors.
- `diff/parser.rs`, `diff/hunk.rs`, `diff/discovery.rs`, `diff/commands.rs`,
  `diff/alignment.rs` (including agreement with a reference
  implementation of line pairing), `sync/classifier.rs`,
  `sync/debounce.rs`, `sync/cache.rs` (eviction checked against a model),
  `sync/watcher.rs`, `tui/keymap.rs`, `tui/layout.rs`.

## Integration tests (real repositories)

Each test builds its own repositories with the real `git` executable and
asserts on the parsed model or the CLI output, never just on exit codes.

| File | Covers |
|---|---|
| `git/discovery.rs` | Root, nested levels, `.git` dir vs file, linked and multiple worktrees, spaces/Unicode in repo paths, shallow clone, non-repo, bare repo, `.git` dir, missing path |
| `git/status.rs` | Clean; `.M M. MM MD A. AM AD .A D. .D R. RM RD .R C. .T T.`; chmod; multiple/nested untracked; ignored files; tracked-but-ignored; submodule `S` field; combined matrix counts |
| `git/branches.rs` | Normal, detached, empty, unborn with custom name, orphan, slash and unusual names, external switches, HEAD hash |
| `git/upstream.rs` | Available, slash branches, none, no remote, never-fetched ref, deleted-and-pruned upstream, local-branch upstream, detached, unborn with upstream |
| `git/tracking.rs` | Identical, ahead 1/3, behind 1/4, diverged 2/3, merge commit, back-dated commits, no upstream, missing ref, stale ref, deleted upstream, detached, shallow — every count cross-checked with `git rev-list --left-right --count` |
| `git/states.rs` | UU; UU+UD+DU+AA at once; DD/AU/UA from a rename/rename conflict; conflict resolution; rebase conflict (detached); full snapshot coherence; empty repository snapshot |
| `git/filenames.rs` | Spaces, leading/trailing spaces, tab, newline, CR, quotes, backslash, `#;&%`, leading dash, ` -> `, glob characters, NFC Unicode, CJK, emoji — as untracked, staged, modified, and renamed (both paths); non-UTF-8 names; CLI escaping |
| `git/errors.rs` | Not a repo, broken `.git` pointer, corrupt index, corrupt HEAD, Git missing from PATH, Git not executable, permission-denied directories, nonexistent directory, malformed output, expected states that are not errors |
| `cli/inspect.rs` | Help/version/default, usage errors, and output for clean, dirty, conflicted, detached, empty, no-upstream, missing-upstream, nested, and linked-worktree cases, plus a closed stdout pipe |
| `git/read_only.rs` | Byte-level before/after fingerprints with a positive control, partial-clone no-fetch with a positive control, a logging Git wrapper that audits every invocation, and a source audit of spawn sites |

### Temporary repositories

`tests/support/mod.rs` provides `TempDir` (a unique directory under
`$TMPDIR/diffkit-tests`, removed on drop), `Repo` (init, bare init, clone,
write, remove, commit, rev-parse, `rev_list_counts`, snapshot), `Remote` (a
work repository tracking a local bare remote, plus a second clone that can
publish commits), and helpers to run the built binary.

Isolation: on first use the test process sets `GIT_CONFIG_GLOBAL` to the
null device, sets `GIT_CONFIG_NOSYSTEM=1`, caps discovery at the temp base
with `GIT_CEILING_DIRECTORIES`, and clears `GIT_DIR`-style variables. Each
repository gets a local test identity. The developer's configuration,
identity, SSH setup, and repositories are never used.

### Local bare remotes

Remote scenarios use `git init --bare` on disk and `file://` URLs. "Behind"
and "stale" states are produced by pushing from the second clone. Nothing
touches the network.

## Edge cases that depend on the platform

- Non-UTF-8 filenames: APFS (macOS) refuses to create them, so
  `non_utf8_filename_on_disk_where_filesystem_allows` logs `SKIPPED` there.
  `non_utf8_filename_from_real_git_index` puts such a name into the index
  with `git update-index --cacheinfo`, producing genuine Git output with raw
  bytes on every Unix filesystem.
- Symlink type changes, executable-bit changes, and permission tests are
  `#[cfg(unix)]`. The permission test skips itself when run as root.
- Windows: the code compiles and passes clippy for `x86_64-pc-windows-msvc`,
  but the tests have not been run on Windows.

## Regression testing

The whole suite runs after every change. Cross-cutting checks include:
counts from `status::read` equal those in the full snapshot (upstream
tracking does not alter status); ordinary filenames are asserted alongside
unusual ones; worktree tests also assert the main repository is unaffected.

## Performance measurement

`examples/bench_inspect.rs` builds deterministic repositories with 100,
5,000, and 50,000 tracked files, each tracking a local bare remote. It
times the release binary over warm-up and measured runs, clean and dirty;
see `PERFORMANCE_BUDGET.md`.

## Phase 2: diff engine tests

### Unit tests (`src/diff/**`)

- `parser`: single and multiple files and hunks; all hunk header forms
  (missing count = 1, zero-line, `@@ -0,0`, `+5,0`); additions and deletions
  with `/dev/null`; empty files; rename, copy, and dissimilarity metadata;
  mode-only changes; binary notices; no-newline markers; C-quoted paths
  (octal escapes, `\t`, `\n`, `\"`); the space-marker tab after plain and
  quoted names; mixed quoted/unquoted headers; non-UTF-8 paths and content;
  content lines that look like headers; `diff --cc` and `* Unmerged path`
  sections; preservation of unknown headers; 12 malformed or truncated inputs;
  error line numbers.
- `alignment`: single and multiple line replacement; insertion only;
  deletion only; unrelated lines not paired; reordering; repeated lines
  (deterministic and minimal); empty lines and CRLF vs LF; identical and
  empty inputs; insertion before a modification; a 20,000-line block with no
  common lines; a 3,000-line block with every 7th line changed; the explicit
  edit-cost bound (bounded, deterministic, valid script); and **Myers
  minimality on 2,000 random inputs against an O(N·M) edit-distance oracle**
  (the oracle exists only in tests).
- `hunk`: independent old/new numbering, zero-count hunks, CRLF/LF/no-EOL
  decoding, `\r` without newline, overlapping hunks rejected, line splitting,
  synthesized addition headers.
- `discovery`: raw/numstat pairing, paths with tabs/newlines/colons,
  stat-only records dropped and counted, malformed output, unmerged U+M
  folding, kind classification (ModeChanged, TypeChanged).
- `commands`, `binary`, `errors`: argument shapes per comparison, pathspecs
  after `--`, the binary heuristic (NUL in 8000 bytes; non-UTF-8 is not
  binary), error messages.

### Integration tests (real Git, temporary repositories)

| File | Covers |
|---|---|
| `diff/comparisons.rs` | Clean repository; partial staging asserting HEAD→index, index→worktree, HEAD→worktree rows, line numbers, and object IDs; combined ≠ sum; independent line numbers; staged addition and deletion; unstaged deletion; renames with and without edits; copies; mode-only and mode+content changes; separated hunks with context 0/3/20; multiple files; unborn HEAD (empty-tree base); a 50,000-line file with 50 hunks; exact subprocess counts; stat-only files dropped without an index write |
| `diff/special_files.rs` | Binary add, modify, and delete (with sizes); symlink target change; file → symlink type change; empty files; missing final newline; CRLF and mixed endings; non-UTF-8 content; conflicts in staged and unstaged (`Unmerged`, `load_all` clean) and combined (flagged); submodule commit and dirty state |
| `diff/untracked.rs` | Text, empty, no-EOL, CRLF, binary, Unicode, nested; ignored files and `info/exclude`; no index or object writes; size limit; vanished and changed files; symlinks not followed; directory swapped for an outside symlink refused; unreadable files; nested repositories; non-UTF-8 names (skipped on APFS) |
| `diff/lazy_loading.rs` | Discovery metadata vs loaded content; restaged file (`ObjectChanged`); edited or deleted working-tree file (`WorkingTreeChanged`); reverted change (`MissingFromDiff`); `load_all` equals per-file loads and reports new changes; unknown `FileId`; per-file and total patch limits with fallback |
| `diff/paths.rs` | 13 hostile names (spaces, tab, newline, quotes, backslash, leading dash, ` -> `, glob characters, pathspec magic `:(attr:…)`, `:!`, Unicode, emoji) in all three comparisons and through `load_all`; renames keeping both unusual paths; a non-UTF-8 tracked path via the index; a portable Unicode subset |
| `diff/read_only.rs` | Byte-level fingerprints unchanged after `diff-stats --load` and every API call; `diff.external` and textconv never run, with a positive control; Git invocation audit of `diff-stats --load` |
| `cli/diff_stats.rs` | `diffkit diff-stats` and `--load` output; errors and usage |

Platform-dependent coverage: on-disk non-UTF-8 names log `SKIPPED` on APFS.
Non-UTF-8 tracked paths are exercised through the index instead.
Permission tests skip as root. Windows code compiles and passes clippy for
`x86_64-pc-windows-msvc`, but has not been run.

Benchmarks: `examples/bench_diff.rs` (see `PERFORMANCE_BUDGET.md`).

## Phase 3: terminal interface tests

- **Unit tests:**
  - `display`: tab expansion, control pictures for C0/DEL, escapes for
    C1/bidi/invalid bytes, path escaping of format controls,
    width-aware truncation.
  - `tui::layout`: too-small sizes, tiling, sidebar bounds, column widths,
    narrow-panel requirement.
  - `tui::keymap`: every documented binding; modifiers and key releases
    don't alias; overlays capture keys.
- **`tests/tui/app.rs` (17, real repositories):**
  - Startup runs 5 Git calls (the untracked list comes from status) and
    loads no diff; the first load is 1 call.
  - Selection bounds; empty repository.
  - Comparison switching with partial staging (A→B, B→C, A→C rows).
  - Per-comparison selection memory.
  - Vertical and horizontal scroll bounds.
  - Hunk navigation, with and without hunks.
  - Help overlay and status dismissal.
  - Refresh keeping the selection by identity, and picking up external
    staging.
  - Stale file reported and not shown.
  - Cache reuse (0 Git calls) and Enter reload (1 call).
  - **Rendering, scrolling, resizing, and overlays run 0 Git calls.**
  - Resizing keeps scroll valid; sidebar hide and resize.
  - A full flow over rename, deletion, binary, conflict, and untracked files
    in all four modes.
- **`tests/tui/render.rs` (14, `TestBackend`):**
  - All regions at 120×40 and 80×24.
  - Numbers, markers, and colors of context, modified, and inserted rows.
  - One-sided added and deleted files.
  - No-newline and CRLF markers.
  - Binary, conflict (unstaged and combined), mode change, rename, and
    empty-file notices.
  - Narrow fallback; too-small warning.
  - No panic over 99 sizes × overlays × scrolling.
  - 150-file sidebar; long-name truncation.
  - Horizontal scrolling with fixed gutters.
  - Escape sequences in content and filenames never reach the buffer.
  - Header tracking and detached states.
- **`tests/tui/terminal.rs` (4):**
  - Argument routing without a TTY.
  - Real pseudo-terminal sessions through `script(1)` (macOS/Linux): launch,
    keys, quit with exit 0, the alternate screen left and cursor shown last,
    no cursor-position query, explicit path argument, and repository errors
    printed before the terminal is touched.
- **`tests/panic_hook.rs` (1):** the panic hook chains to the previous hook
  (its own binary, since hooks are global).

Phase 1's `cli::inspect::help_version_and_default` was updated for the one
intended behavior change: `diffkit` with no arguments now opens the viewer,
and without a terminal exits 1 with a message. The `--help`, `-h`, and
`--version` assertions are unchanged.

Benchmarks: `examples/bench_tui.rs`.

## Phase 4: synchronization tests

- **Unit tests (`src/sync/**`):**
  - `events`: set operations.
  - `classifier` (7 tests): working-tree create, modify, remove, rename,
    and atomic replace; reads ignored; every Git metadata class; objects,
    logs, hooks, and FETCH_HEAD ignored; Git-ignored and nested-repository
    paths ignored, but a `target` that is not ignored is kept; linked
    worktree metadata (own vs main worktree's); rescan becomes OVERFLOW;
    root removal becomes STRUCTURE.
  - `debounce` (6 tests, deterministic clock): single event, duplicates and
    bursts, continuous activity firing at the maximum delay, empty input,
    forced take, maximum ≥ quiet.
  - `cache` (6 tests): hits, misses, LRU, budget with the protected entry,
    targeted revalidation, rename and comparison identity, full
    invalidation, size accounting.
  - `watcher`: a full queue counts drops and never blocks; watch plans for
    standard and linked repositories.
  - `reconcile`: reuse rules.
- **`tests/sync/scheduler.rs` (8, scripted reconciler, no watcher):**
  - One run for a 100-event burst (99 coalesced).
  - Events during a run are kept and the next run includes them; never two
    runs at once.
  - Manual refresh skips the debounce.
  - The maximum delay prevents starvation.
  - A failure is reported with no retry loop.
  - Dropped events force a full OVERFLOW run.
  - Shutdown during the debounce is prompt; shutdown never hangs on a busy
    worker.
- **`tests/sync/reconcile.rs` (5, real Git):**
  - A refs-only change reuses all four lists (2 Git calls).
  - A working-tree change reuses only Staged.
  - A moved HEAD never reuses.
  - INDEX, MANUAL, and OVERFLOW rediscover.
  - Ignored directories: only directories that are themselves ignored,
    including a pathspec-magic-looking name.
- **`tests/sync/watch.rs` (11, real watcher + Git):**
  - Editing; staging and unstaging; untracked files appearing and
    disappearing; deletion and rename.
  - External commit and branch switch; conflict introduced and resolved.
  - A 100-write burst gives ≤ 3 reconciliations.
  - Ignored build output and idle Git reads cause no reconciliation.
  - Repository removal degrades without crashing.
  - Linked worktree: its tree, its HEAD, and a shared ref moved from the
    main worktree.
  - `.git` stays byte-for-byte unchanged while syncing.
- **`tests/tui/sync.rs` (10):**
  - Unrelated changes keep the selection and cached diffs (1
    invalidation; 0 Git calls to show the kept file).
  - The viewed file changing reloads it and keeps the scroll position.
  - A vanished selection moves to the nearest file with a notice.
  - The header follows branch switches.
  - Every indicator state; degraded mode keeps manual refresh through the
    worker; `--no-watch` label.
  - Cache budget evicts but protects the shown diff, and reloads.
  - Diagnostics overlay.
  - The interface stays responsive (200 actions and frames) while a
    reconciliation is blocked.
- **`tests/tui/terminal.rs` (+2 real-PTY sessions):**
  - An external edit and a new file appear without `r`; `Synced` shown;
    the terminal is restored; exit within 3 s.
  - A Git audit of the running viewer (edits, an external `git add`, `r`):
    only allowed read-only subcommands, `GIT_OPTIONAL_LOCKS=0` on every
    call.

Changed Phase 3 assertion: `tui::app::refresh_keeps_selection_by_identity`
expected refresh to clear the cache. It now expects unchanged files to keep
their cached diffs, which is Phase 4's targeted invalidation.

Timing: debounce logic is tested with injected instants. Thread and
watcher tests wait for conditions with generous deadlines (5–15 s) rather
than fixed sleeps. `sync_scheduler` and `sync_watch` were each run 6 times
in a row without failure.

Benchmarks: `examples/bench_sync.rs`.

## Phase 5: advanced states, history, and release tests

Every new behaviour is tested against real temporary repositories. New
tests wrap inspection in `common::assert_read_only`, which fingerprints
every file and symlink under the repository (`.git` included: index,
refs, HEAD, objects, state files) before and after, and fails on any
difference.

- **Unit tests (12 new):**
  - conflict-stage parsing (all stages, a malformed stage ID is an error);
  - stage → conflict-kind mapping for all seven shapes;
  - `git::state` (none, rebase with steps, apply vs `am`, merge,
    cherry-pick, revert, bisect, sequencer, oversized or garbage files);
  - `git log` record parsing (merges, multi-line messages, shallow
    boundary, malformed records);
  - commit-range arguments;
  - bisect classification;
  - the `inspect` report's operations and conflict stages.
- **`tests/git/conflicts.rs` (5):**
  - `UU`, `AA`, `UD`, and `DU` stages match `git ls-files --unmerged`
    exactly;
  - working-tree presence;
  - conflicts versus ordinary edits;
  - `inspect` stages;
  - the interface's conflict panel.
- **`tests/git/renames.rs` (5):**
  - renames across directories, with and without edits, in Staged and
    Combined;
  - an unrelated delete + add is not a rename;
  - unusual names (spaces, Unicode, a leading dash, pathspec magic, tab,
    newline);
  - renames in commits;
  - a case-only rename on a case-insensitive file system.
- **`tests/git/submodules.rs` (5):** moved submodule commit,
  uninitialized submodule, missing directory, embedded repository
  (gitlink without `.gitmodules`), untracked nested repository. Files
  inside are never listed and the submodule is never initialized.
- **`tests/git/symlinks.rs` (4, Unix):**
  - target change;
  - broken links, tracked and untracked;
  - a link to a secret outside the repository (only the path is shown,
    never the content);
  - deletion and replacement by a regular file;
  - a regular file holding a path-like string.
- **`tests/git/operations.rs` (11):**
  - merge (and abort);
  - rebase with the sequencer backend (step, branch, onto), interactive
    rebase stopped by `exec`, and the apply backend;
  - cherry-pick and revert;
  - bisect;
  - per-worktree isolation;
  - detached and unborn HEAD;
  - a `.git` file pointing nowhere, and an unreadable `.git` (Unix);
  - the real watcher publishing operation changes.
- **`tests/git/history.rs` (8):**
  - commit metadata checked against Git (Unicode identities, explicit
    dates);
  - every change kind in a commit diff;
  - root and merge commits;
  - limit, order, and detached HEAD;
  - shallow boundary;
  - nine invalid revisions (empty, unknown, nonexistent hex, `-h`,
    `--all`, out of range, a blob, a tree, a range) and four valid ones;
  - `diffkit show` output, escaping, and exit codes;
  - a logging-wrapper audit of every Git process `show` starts.
- **`tests/tui/history.rs` (6):**
  - inspect a commit and return to the identical working-tree view
    (comparison, file, scroll);
  - synchronization while history is open;
  - comparison keys leave history;
  - empty, merge, and shallow-boundary commits;
  - escape sequences in messages and author names;
  - tiny sizes and reload.
- **`tests/tui/states.rs` (3):**
  - bidi and zero-width characters in branch and file names never reach
    a cell;
  - hunk and scroll keys on notice-only views;
  - very long names and subjects at widths 40–250.
- **`tests/sync/platform_paths.rs` (2):** a repository and a linked worktree
  under directories with spaces, accents, CJK, and emoji, through the
  interface, the CLI, and the real watcher.
- **`tests/sync/consistency.rs` (1):** a stress test.
  - A thread changes the index, HEAD, and refs in bursts while
    reconciliations run.
  - Every settled result must agree with itself (status versus each list,
    and the base commit).
  - Across five runs, every settled result was consistent, while 12 to 18
    of the roughly 32 unsettled results per run were truly mixed. Those
    are what the guard keeps from being published.
- **`tests/sync/scheduler.rs` (+1):** an unsettled result is withheld and
  followed by a full run.
- **`tests/tui/terminal.rs` (+2 real PTY sessions, 1 extended):**
  - SIGTERM, SIGHUP, and SIGINT restore the terminal and exit with
    128 + N within 3 s;
  - a scripted history session;
  - the Git audit of the running viewer now covers history (`log`,
    `diff-tree`) and checks `--no-ext-diff`, `--no-textconv`, and
    `--no-show-signature` in the arguments.

Changed assertions in earlier tests (none weakened):
- `diff::special_files::conflicts_are_explicit_in_staged_and_unstaged`
  still requires no two-way diff, and now also requires the right kind,
  all three stages, and working-tree presence.
- `tui_app` matches `Notice::Unmerged(_)` instead of `Notice::Unmerged`.
- `tui::render::header_shows_tracking_and_detached_states` expects the new
  wording "(remote-tracking ref, as last fetched)".
- `tui::terminal::interactive_sync_runs_only_read_only_git_commands` gained
  history steps, two allowed subcommands, and argument checks.

PTY transcript matching: differential redraws leave unchanged cells
unwritten, so text drawn over other text can have gaps. The history PTY
test uses a tolerant matcher (a gap may stand for a character; at least
75% must match exactly). It also asserts that a phrase never shown does
not match.

Repetition: the timing-sensitive tests were each run six times in a row
without failure, after one flaky needle was fixed:
- `sync_consistency`, `repository_operations`, `platform_paths`;
- `tui_cli`, `sync_scheduler`, `tui_history`, `sync_watch`.
