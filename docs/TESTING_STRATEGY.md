# Testing Strategy

Run everything with `cargo test`. Tests use only Rust's built-in harness.

## Unit tests (`src/**`, `#[cfg(test)]`)

Pure logic with hand-written inputs, focused on what real Git output cannot
easily produce:

- `git::status` — every record type, rename/copy original-path pairing,
  paths with spaces/tabs/newlines, raw non-UTF-8 bytes (Unix), submodule
  fields, unknown status letters, ignored records, and a table of malformed
  inputs that must return errors instead of panicking.
- `git::branches` — header interpretation for normal, detached, unborn,
  and unknown HEAD; malformed object IDs.
- `git::commits` — `branch.ab` parsing (including `+? -?` and malformed
  values) and the mapping from upstream state to tracking availability.
- `git::models` — staged/unstaged/partial predicates over a table of XY
  codes, counts, tracking relation, object ID validation.
- `git::commands` / `git::errors` — newline stripping that keeps other
  whitespace, path conversion per platform (Windows tests are
  `#[cfg(windows)]`), error messages.
- `app::report` — path escaping and rendering of partially staged files,
  untracked tracking, and the staleness disclaimer.

## Integration tests (`tests/*.rs`)

Each test builds its own repositories with the real `git` executable and
asserts on the parsed model or the CLI output, never just on exit codes.

| File | Covers |
|---|---|
| `repository_discovery.rs` | Root, nested levels, `.git` dir vs file, linked and multiple worktrees, spaces/Unicode in repo paths, shallow clone, non-repo, bare repo, `.git` dir, missing path |
| `git_status.rs` | Clean; `.M M. MM MD A. AM AD .A D. .D R. RM RD .R C. .T T.`; chmod; multiple/nested untracked; ignored files; tracked-but-ignored; submodule `S` field; combined matrix counts |
| `branch_detection.rs` | Normal, detached, empty, unborn with custom name, orphan, slash and unusual names, external switches, HEAD hash |
| `upstream_detection.rs` | Available, slash branches, none, no remote, never-fetched ref, deleted-and-pruned upstream, local-branch upstream, detached, unborn with upstream |
| `commit_tracking.rs` | Identical, ahead 1/3, behind 1/4, diverged 2/3, merge commit, back-dated commits, no upstream, missing ref, stale ref, deleted upstream, detached, shallow — every count cross-checked with `git rev-list --left-right --count` |
| `repository_states.rs` | UU; UU+UD+DU+AA at once; DD/AU/UA from a rename/rename conflict; conflict resolution; rebase conflict (detached); full snapshot coherence; empty repository snapshot |
| `unusual_filenames.rs` | Spaces, leading/trailing spaces, tab, newline, CR, quotes, backslash, `#;&%`, leading dash, ` -> `, glob characters, NFC Unicode, CJK, emoji — as untracked, staged, modified, and renamed (both paths); non-UTF-8 names; CLI escaping |
| `error_handling.rs` | Not a repo, broken `.git` pointer, corrupt index, corrupt HEAD, Git missing from PATH, Git not executable, permission-denied directories, nonexistent directory, malformed output, expected states that are not errors |
| `cli_inspect.rs` | Help/version/default, usage errors, and output for clean, dirty, conflicted, detached, empty, no-upstream, missing-upstream, nested, and linked-worktree cases, plus a closed stdout pipe |
| `read_only.rs` | Byte-level before/after fingerprints with a positive control, partial-clone no-fetch with a positive control, a logging Git wrapper that audits every invocation, and a source audit of spawn sites |

### Temporary repositories

`tests/common/mod.rs` provides `TempDir` (a unique directory under
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
| `diff_comparisons.rs` | Clean repository; partial staging asserting HEAD→index, index→worktree, HEAD→worktree rows, line numbers, and object IDs; combined ≠ sum; independent line numbers; staged addition and deletion; unstaged deletion; renames with and without edits; copies; mode-only and mode+content changes; separated hunks with context 0/3/20; multiple files; unborn HEAD (empty-tree base); a 50,000-line file with 50 hunks; exact subprocess counts; stat-only files dropped without an index write |
| `diff_special_files.rs` | Binary add, modify, and delete (with sizes); symlink target change; file → symlink type change; empty files; missing final newline; CRLF and mixed endings; non-UTF-8 content; conflicts in staged and unstaged (`Unmerged`, `load_all` clean) and combined (flagged); submodule commit and dirty state |
| `diff_untracked.rs` | Text, empty, no-EOL, CRLF, binary, Unicode, nested; ignored files and `info/exclude`; no index or object writes; size limit; vanished and changed files; symlinks not followed; directory swapped for an outside symlink refused; unreadable files; nested repositories; non-UTF-8 names (skipped on APFS) |
| `diff_lazy_loading.rs` | Discovery metadata vs loaded content; restaged file (`ObjectChanged`); edited or deleted working-tree file (`WorkingTreeChanged`); reverted change (`MissingFromDiff`); `load_all` equals per-file loads and reports new changes; unknown `FileId`; per-file and total patch limits with fallback |
| `diff_paths.rs` | 13 hostile names (spaces, tab, newline, quotes, backslash, leading dash, ` -> `, glob characters, pathspec magic `:(attr:…)`, `:!`, Unicode, emoji) in all three comparisons and through `load_all`; renames keeping both unusual paths; a non-UTF-8 tracked path via the index; a portable Unicode subset |
| `diff_read_only.rs` | Byte-level fingerprints unchanged after `diff-stats --load` and every API call; `diff.external` and textconv never run, with a positive control; Git invocation audit of `diff-stats --load` |
| `diff_cli.rs` | `diffkit diff-stats` and `--load` output; errors and usage |

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
- **`tests/tui_app.rs` (17, real repositories):**
  - Startup runs 6 Git calls and loads no diff; the first load is 1 call.
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
- **`tests/tui_render.rs` (14, `TestBackend`):**
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
- **`tests/tui_cli.rs` (4):**
  - Argument routing without a TTY.
  - Real pseudo-terminal sessions through `script(1)` (macOS/Linux): launch,
    keys, quit with exit 0, the alternate screen left and cursor shown last,
    no cursor-position query, explicit path argument, and repository errors
    printed before the terminal is touched.
- **`tests/tui_panic.rs` (1):** the panic hook chains to the previous hook
  (its own binary, since hooks are global).

Phase 1's `cli_inspect::help_version_and_default` was updated for the one
intended behavior change: `diffkit` with no arguments now opens the viewer,
and without a terminal exits 1 with a message. The `--help`, `-h`, and
`--version` assertions are unchanged.

Benchmarks: `examples/bench_tui.rs`.
