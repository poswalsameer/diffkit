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
