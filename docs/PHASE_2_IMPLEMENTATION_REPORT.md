# DIFFKIT — PHASE 2 IMPLEMENTATION REPORT

Date: 2026-11-06. Host: macOS 26.5 (arm64), Apple M5, 16 GiB RAM, Git 2.54.0,
rustc/cargo 1.98.1. Baseline: Phase 1 commit `7ad9e49`.

## 1. Executive summary

**Phase 2 is complete on the tested platform (macOS/Unix).** Windows code
compiles and passes clippy for `x86_64-pc-windows-msvc`, but has not been
executed.

`src/diff/` is a standard-library-only diff engine on top of the Phase 1 Git
layer. It provides:
- Four explicit comparisons (staged, unstaged, combined, untracked).
- Metadata-only discovery: one Git process per comparison.
- Lazy per-file loading: one Git process per file, or zero for untracked
  files.
- `load_all`: one Git process per comparison.
- A byte-oriented unified diff parser and line models that preserve LF,
  CRLF, and missing-newline endings.
- Linear-space, explicitly bounded Myers alignment with a documented
  pairing policy that produces side-by-side rows.
- Explicit binary, submodule, unmerged, too-large, unreadable, and
  nested-repository results.
- Staleness detection between discovery and loading.

`diffkit diff-stats [--load]` exercises the engine from the CLI.

Two significant findings shaped the implementation:

1. **Porcelain `git diff` rewrites `.git/index`** after working-tree
   comparisons, even with `GIT_OPTIONAL_LOCKS=0` or `--no-optional-locks`.
   A read-only fingerprint test caught this. The engine uses the plumbing
   commands `git diff-index` and `git diff-files`, which never write, and
   handles their stat-only entries explicitly.
2. A parser bug in quoted paths with the space-marker tab after the closing
   quote was found by integration tests and fixed. A unit test now pins it.

Totals:
- 236 tests: 87 unit and 149 integration. All pass; 0 failed, 0 ignored.
- All 97 Phase 1 integration tests still pass.
- All five Cargo commands pass with no warnings.
- No dependencies were added. No Phase 3 work was started.

## 2. Final source tree

```text
src/
  main.rs, lib.rs                     lib.rs now exports `diff` and `git`
  app/cli.rs                          + `diff-stats [--load]`
  app/report.rs                       (Phase 1 report; test fixture updated for object_format)
  app/diff_stats.rs                   NEW: diff-engine diagnostic
  git/commands.rs                     + output_limited, thread_invocations, path_bytes, literal pathspecs
  git/repository.rs                   + --show-object-format
  git/models.rs                       + ObjectFormat (empty_tree), ObjectId::is_zero
  git/{branches,commits,errors,mod,snapshot,status}.rs   unchanged
  diff/mod.rs                         NEW: re-exports
  diff/engine.rs                      NEW: DiffEngine
  diff/commands.rs                    NEW: argument arrays
  diff/discovery.rs                   NEW: raw/numstat → DiffFile
  diff/parser.rs                      NEW: unified diff parser
  diff/hunk.rs                        NEW: numbering, endings, validation
  diff/alignment.rs                   NEW: Myers + pairing
  diff/binary.rs                      NEW: binary heuristic, blob sizes
  diff/untracked.rs                   NEW: untracked discovery/reading
  diff/models.rs, diff/errors.rs      NEW
tests/
  (Phase 1: 10 files, unchanged except fixture helpers)
  common/mod.rs                       + diff helpers (engine, discover, entry, load, rows)
  diff_comparisons.rs, diff_special_files.rs, diff_untracked.rs,
  diff_lazy_loading.rs, diff_paths.rs, diff_read_only.rs, diff_cli.rs   NEW
examples/bench_inspect.rs, examples/bench_diff.rs (NEW)
docs/ ARCHITECTURE, DATA_MODEL, ENGINEERING_RULES, TECH_STACK, TESTING_STRATEGY,
      PERFORMANCE_BUDGET (updated), phases/PHASE_2_DIFF_ENGINE.md (rewritten),
      PHASE_2_IMPLEMENTATION_REPORT.md (this file)
```

## 3. Architecture and module responsibilities

| Module | Responsibility | Does I/O |
|---|---|---|
| `diff::engine` | Public API; orchestration; working-tree, object ID, line count, and binary staleness checks; type-change merging | Git, `lstat` |
| `diff::commands` | Argument arrays for `diff-index`, `diff-files`, `ls-files`, `cat-file` | no |
| `diff::discovery` | Parses `--raw --numstat -z`; pairs records by path; drops stat-only entries; folds unmerged U+M; classifies kinds | `lstat` only |
| `diff::parser` | Byte-oriented unified diff parser | no |
| `diff::hunk` | Hunk validation, independent numbering, line-ending decoding, untracked addition hunks | no |
| `diff::alignment` | Myers shortest edit script (linear space, bounded) and row pairing | no |
| `diff::binary` | NUL heuristic for untracked files; `cat-file -s` sizes | Git |
| `diff::untracked` | `ls-files` discovery; safe, size-limited reading | Git, file system |
| `diff::models` / `diff::errors` | Data model and `DiffError` | no |
| `git::commands` | Single spawn site (Phase 1), now with `output_limited` | spawns Git |

All processes still start from the single `Command::new(GIT)` in
`git/commands.rs`, enforced by `read_only::only_the_command_layer_starts_processes`.

## 4. Git commands used

All run as `git -C <root> …` with `GIT_OPTIONAL_LOCKS=0`,
`GIT_NO_LAZY_FETCH=1`, `GIT_LITERAL_PATHSPECS=1`, the glob/icase/`GIT_DIFF_OPTS`
variables removed, and stdin closed.

Common diff options (`C`): `--no-ext-diff --no-textconv --no-color
--no-relative --find-renames --find-copies --diff-algorithm=histogram
--submodule=short`.

| Purpose | Command | Read-only |
|---|---|---|
| Staged discovery | `diff-index --cached C --raw --numstat -z --no-abbrev <base>` | yes |
| Unstaged discovery | `diff-files C --raw --numstat -z --no-abbrev` | yes |
| Combined discovery | `diff-index C --raw --numstat -z --no-abbrev <base>` | yes |
| Patch (one file or all) | same command, `C --patch --full-index --src-prefix=a/ --dst-prefix=b/ --inter-hunk-context=0 -U<n> [<base>] -- [paths…]`; output bounded | yes |
| Untracked discovery | `ls-files --others --exclude-standard -z` | yes |
| Binary sizes | `cat-file -s <oid>` (repository sides of binary files only) | yes |
| Engine setup (`open`) | Phase 1 snapshot: `rev-parse …`, `status --porcelain=v2 …` (+ optional `rev-parse`/`for-each-ref`) | yes |

Never run: porcelain `diff`, `add`, `hash-object`, `checkout`, `restore`,
`reset`, `apply`, `fetch`, or any external diff or textconv program.
Evidence: `diff_read_only::diff_stats_runs_only_allowed_git_commands` and
`diff_read_only::external_diff_and_textconv_are_never_run`.

## 5. Comparison semantics

| `DiffComparison` | Meaning | Command |
|---|---|---|
| `Staged` | HEAD → index | `diff-index --cached <base>` |
| `Unstaged` | index → working tree | `diff-files` |
| `Combined` | HEAD → working tree (tracked) | `diff-index <base>` |
| `Untracked` | → untracked file | `ls-files --others --exclude-standard`, then read |

`<base>` is the snapshot's HEAD commit, or the empty tree when HEAD is
unborn. These are the same comparisons as `git diff --cached`, `git diff`,
and `git diff HEAD`, through the commands that do not write the index.

Evidence: `diff_comparisons::partial_staging_keeps_three_comparisons_distinct`
uses HEAD = "value = A", index = "B", working tree = "C". It asserts the rows
Context/Modified(A→B)/Context, then Modified(B→C), then Modified(A→C), with
line numbers and object IDs. `combined_is_not_the_sum_of_staged_and_unstaged`
stages an insertion and then removes it in the working tree: Staged +1,
Unstaged −1, Combined empty.

## 6. Data models

Fully documented in `docs/DATA_MODEL.md`. In summary:
- `DiffFileList { comparison, base, files, stat_only_entries }`.
- `DiffFile`: id, paths, kind, modes, similarity, object IDs (only those Git
  printed), binary status, numstat counts, conflicted flag, and
  working-tree stamp.
- `FileDiff { file, content, source, old_oid, new_oid }`.
- `DiffContent`: Text, Binary, Submodule, Unmerged, TooLarge, Unreadable,
  or NestedRepository.
- `DiffHunk { old_start, old_count, new_start, new_count, header,
  section_heading, rows, alignment }`.
- `AlignedRow { kind, old_line: Option<DiffLine>, new_line: Option<DiffLine> }`.
- `DiffLine { content: Vec<u8>, line_number, ending: Lf | CrLf | None }`.

There are no strings for missing lines and no invented object IDs or
similarities.

## 7. Unified diff parser

`src/diff/parser.rs`. Input is split on `\n` only; everything is bytes.

- Recognizes `diff --git`, `---`/`+++` (`/dev/null`, C-quoted, and the tab
  Git appends to names with spaces, including after a closing quote),
  `old mode`/`new mode`, `new file mode`/`deleted file mode`,
  `similarity index`/`dissimilarity index`, `rename from/to`,
  `copy from/to`, `index <a>..<b>[ mode]`, `Binary files … differ`, hunks,
  and `\ No newline at end of file`.
- `diff --cc`/`--combined` sections and `* Unmerged path` notices are
  recognized and skipped as unmerged. Unknown header lines are kept in
  `extended_headers`.
- Hunk headers accept every form (`-10 +10`, `-0,0 +1,3`, `-5,3 +5,0`); a
  missing count means 1.
- Bodies are consumed by count. A truncated hunk, an extra line, or an
  unknown prefix is a `DiffError::Parse { line, detail }`. Overlapping or
  out-of-order hunks are rejected in `hunk.rs`.
- Paths resolve in priority order: rename/copy lines, then `---`/`+++`, then
  the header (only when unambiguous). The engine matches sections to
  discovered files by these resolved byte paths. It never relies on the
  ambiguous unquoted header alone.

Tests: 20 parser unit tests (including 12 malformed or truncated inputs) and
every integration test that loads real Git output.

## 8. Myers algorithm and complexity

`src/diff/alignment.rs` implements the linear-space variant of Myers (1986).
It is divide and conquer on the middle snake, searching forward and backward
simultaneously:
- Time O((N+M)·D), memory O(N+M), with two diagonal vectors reused across
  recursion. There is no N×M matrix.
- Common prefix and suffix are trimmed at each level. Lines are interned to
  integers (content plus line ending).
- If the two sequences share no line, the shortest script is
  delete-then-insert and the search is skipped.
- **Explicit bound:** a middle-snake search stops after
  `max_edit_cost/2 + 1` diagonals (default `max_edit_cost` = 4096). That
  sub-range is emitted as delete-then-insert, and the hunk is marked
  `AlignmentQuality::Bounded`. The bound depends only on input, so results
  are deterministic (`edit_cost_bound_is_explicit_and_deterministic`).
- Correctness: `myers_is_minimal_on_random_inputs` checks validity and
  minimal cost on 2,000 random sequence pairs against an O(N·M)
  edit-distance oracle that exists only in tests.

Git's histogram algorithm decides hunk boundaries. Within a hunk, context
lines are fixed and Myers aligns each block of removed and added lines.
Pairing policy for each delete/insert run between equal lines:
- Each removed line looks at the next 16 unpaired added lines and pairs
  with the most similar one (ties go to the earliest) if similarity is at
  least 40%.
- Similarity is the bigram Dice coefficient of the trimmed lines, over at
  most the first 512 bytes.
- Skipped added lines become `Added`; unmatched removed lines become
  `Removed`. Both columns stay ordered.

## 9. Line alignment examples (from tests)

| Old | New | Rows |
|---|---|---|
| `const name = "Alice";` / `const age = 20;` / `console.log(name);` | …`21`… | Context(1,1), Modified(2,2), Context(3,3) |
| `a`, `c` | `a`, `b`, `c` | Context, Added(—,2), Context(2,3) |
| `fn alpha() {}` | `struct Zed;` | Removed(1,—), Added(—,1): unrelated lines are not paired |
| `let total = a + b;` | `// new comment`, `let total = a + b + c;` | Added(—,1), Modified(1,2) |
| `x\r\n` | `x\n` | Modified: same text, ending changed |
| Hunk `@@ -10,4 +20,4 @@` with ` a / -let b = 1; / +let b = 2; / +new / c / -d` | | Context(10,20), Modified(11,21), Added(—,22), Context(12,23), Removed(13,—) |

## 10. Rename and copy support

- `--find-renames --find-copies` at discovery. Raw `R<score>`/`C<score>`
  records give both paths and the similarity.
- Per-file loads pass both paths after `--` so Git pairs them again. The
  section is matched by (old path, new path).
- Pure renames have no hunks (`has_hunks() == Some(false)`); edited renames
  carry hunks with correct line numbers.

Tests: `rename_with_and_without_modification`,
`copy_is_reported_when_git_detects_it`, and
`renames_keep_both_unusual_paths`, which renames 13 hostile names to targets
containing a tab.

## 11. Binary handling

- Binary status comes from numstat `-`/`-` and the patch's `Binary files …
  differ`. Binary content is never decoded or stored.
- `BinarySummary` sizes come from `cat-file -s` for repository objects and
  from file size for working-tree sides. Untracked files use a NUL in the
  first 8000 bytes (Git's heuristic).
- A change in binary classification between discovery and load is
  `Stale(BinaryChanged)`.

Tests: `binary_addition_modification_and_deletion` asserts exact sizes for
add, modify, and delete; `untracked_contents_as_additions` covers
`data.bin`.

## 12. Untracked handling

- `ls-files --others --exclude-standard -z` applies `.gitignore`,
  `info/exclude`, and `core.excludesFile`. Each file is read directly.
- Nothing is staged or hashed: `reading_untracked_files_does_not_touch_the_repository`
  compares the index bytes and `count-objects`.
- Safety:
  - Normal path components only.
  - The parent must resolve inside the root (`directory_swapped_for_outside_symlink_is_refused`).
  - Symlinks are not followed; their target is the content (`symlinks_are_not_followed`).
  - The opened file must match the `lstat`ed one, and metadata must be
    unchanged after reading.
- Results are distinct: `TooLarge` (limit 8 MiB by default, file not read),
  `Unreadable`, `Binary`, `NestedRepository`, `Vanished`, and `Stale`.

## 13. Lazy loading

- `discover` returns metadata only. Its single Git call is asserted in
  `subprocess_counts`.
- `load_file_diff` makes one Git call limited to that file's paths, or no
  call for untracked and unmerged files.
- `load_all` makes one Git call per comparison. If output exceeds
  `max_total_patch_bytes` it falls back to per-file loads; new changes Git
  reports appear in `unexpected_paths`.
- Nothing is cached. Every load is verified against discovery
  (working-tree stamp before and after, object IDs, line counts, binary
  flag, presence), giving `Stale { reason }` when they differ.

Tests: `restaged_file_is_stale`,
`unstaged_file_edited_or_deleted_after_discovery_is_stale`,
`change_reverted_after_discovery_is_missing`,
`load_all_matches_per_file_loads_and_reports_new_changes`, and
`patch_size_limits`.

## 14. Path and encoding handling

- Discovery paths come from NUL-separated `-z` output, converted by
  `path_from_git` (raw bytes on Unix, strict UTF-8 on Windows). They are
  passed back to Git as `OsStr` after `--`, never through a lossy string.
- `GIT_LITERAL_PATHSPECS=1` makes names such as `:(attr:nope)magic.txt`,
  `:!excluded.txt`, and `glob*?[ab].txt` literal.
- Patch header paths are C-unquoted (octal escapes included) and compared as
  bytes.
- Content is `Vec<u8>`; non-UTF-8 bytes stay intact
  (`non_utf8_content_is_kept_as_bytes`). A non-UTF-8 tracked path is
  exercised through the index (`non_utf8_tracked_path`).
- On-disk non-UTF-8 untracked names cannot be created on APFS; that test
  logs `SKIPPED` on this host.

## 15. Performance benchmarks

Full tables and individual runs are in `docs/PERFORMANCE_BUDGET.md`
(`cargo run --release --example bench_diff -- 7`, release build, 2 warm-up
and 7 measured runs).

| Tracked | Changed (unstaged) | Discover | Load 1 | `load_all` | Git calls | diffkit footprint |
|---:|---:|---:|---:|---:|---|---:|
| 100 | 1 | 3.94 ms | 3.21 ms | 3.16 ms | 1/1/1 | 1.2 MiB |
| 5,000 | 50 | 8.31 ms | 3.45 ms | 8.70 ms | 1/1/1 | 2.7 MiB |
| 50,000 | 1 | 35.57 ms | 5.23 ms | 35.56 ms | 1/1/1 | 1.2 MiB |
| 50,000 | 500 | 51.81 ms | 5.17 ms | 55.13 ms | 1/1/1 | 9.1 MiB |

| Single file | Load | of which Git | Parse | Hunks + alignment |
|---|---:|---:|---:|---:|
| 200k-line text, 2,000 lines edited | 521.20 ms | 517.15 ms | 0.71 ms | 1.35 ms |
| 20 MiB binary | 40.47 ms | 37.12 ms | — | — |
| 20k lines rewritten, no common lines | 59.27 ms | 9.24 ms | 1.07 ms | 49.44 ms |
| 20k lines, every 3rd changed | 234.20 ms | 230.30 ms | 0.65 ms | 3.03 ms |

Not measured: Linux, Windows, cold caches, debug builds, and
fsmonitor/untracked-cache setups.

## 16. Unit test results

87 unit tests: 82 in the library and 5 in the binary. All pass, 0 ignored.

- Phase 2 added 49 `diff::*` tests: `parser` 20, `alignment` 13, `hunk` 7,
  `discovery` 5, `commands` 2, `binary` 1, `errors` 1.
- Two new `git::commands` tests cover limited output and the invocation
  counter.
- One `#[cfg(windows)]` test exists but has not been run.

## 17. Integration test results

149 integration tests in 17 files. All pass; 0 failed, 0 ignored.

- **Phase 1:** 97 tests, unchanged in behavior.
- **Phase 2:** 52 tests:
  - `diff_comparisons` 15
  - `diff_special_files` 10
  - `diff_untracked` 10
  - `diff_lazy_loading` 7
  - `diff_paths` 4
  - `diff_read_only` 3
  - `diff_cli` 3
- **Runtime skips:**
  - `diff_untracked::non_utf8_untracked_name_where_filesystem_allows` and
    Phase 1's `unusual_filenames::non_utf8_filename_on_disk_where_filesystem_allows`
    print `SKIPPED` on APFS.
  - The permission tests skip only when run as root (they ran here).

Test fixes made during development were all corrections of wrong
expectations or fixture setup, not weakened assertions:
- Two hunk-count typos in byte fixtures; the parser correctly rejected them.
- `git mv` into a missing directory.
- Git's real `@@ … @@ l1` function-context heading.
- Miscounted fixture byte lengths, now computed from the bytes.
- Three pairings expected to be `Modified` where the documented 40%
  threshold does not pair them (for example `two` vs `two changed` scores
  33).
- One real engine bug, the quoted-path tab, was fixed in code.

## 18. All five validation commands

Run after the final code change, in order, on 2026-11-06:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check` | exit 0, 0 warnings |
| `cargo build --release` | exit 0, 0 warnings |
| `cargo test` | exit 0; 236 passed, 0 failed, 0 ignored |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0, 0 warnings |
| Extra: `cargo clippy --target x86_64-pc-windows-msvc --all-targets --all-features -- -D warnings` | exit 0 (compile and lint only) |

Clippy findings during development were fixed in code, not suppressed:
- Type aliases (`PathPair`, `SidePath`).
- `then_some`, `is_multiple_of`, and a needless `format!`.
- An elided lifetime.
- `#[cfg(unix)]` on Unix-only test imports.

No new `allow` attributes were added.

## 19. Read-only verification

- **Fingerprints:** `diff_read_only::diff_engine_leaves_every_file_untouched`
  compares bytes and mtimes of every file under the fixture (`.git`
  included) after two rounds of `diff-stats --load` plus every API call in
  every comparison. The fixture is mid-merge with a conflict, staged and
  partially staged files, an untracked directory, and a stat-dirty file. It
  initially **failed** on `.git/index`, which led to the switch to plumbing.
  Now no file changes.
- **Stat-only files:** `stat_only_changes_are_not_reported_as_modifications`
  checks that a touched file is not reported and the index stays identical.
- **No external programs:** `external_diff_and_textconv_are_never_run`
  configures `diff.external` and a textconv driver that write a marker file
  and uppercase content. After loading all three comparisons and running the
  CLI, no marker exists and no uppercase content appears. Control: plain
  `git diff --cached` creates the marker.
- **Command audit:** `diff_stats_runs_only_allowed_git_commands` puts a
  logging wrapper on PATH. Only `rev-parse`, `status`, `for-each-ref`,
  `diff-index`, `diff-files`, `ls-files`, and `cat-file` run. Every diff
  call has `--no-ext-diff --no-textconv --no-color`; every call has
  `GIT_LITERAL_PATHSPECS=1` and `GIT_OPTIONAL_LOCKS=0`. Exactly 6 diff calls
  (3 discoveries and 3 `load_all`s), 1 `ls-files`, and 1 `status`.
- **Untracked reads:** `reading_untracked_files_does_not_touch_the_repository`
  confirms the index bytes and object count are unchanged and the file is
  still untracked.
- **Real repository:** `diffkit diff-stats --load` on this repository
  (16 unstaged files, many untracked) left every non-object file in `.git`
  byte-identical and the object store unchanged.
- **No network:** no network-capable command is run, and
  `GIT_NO_LAZY_FETCH=1` still applies.

## 20. Dependencies

None. `Cargo.toml` has no dependencies and `Cargo.lock` lists only
`diffkit`. The parser, Myers, and similarity are hand-written with `std`.

## 21. Known limitations

1. **Windows is not executed.**
   - `path_bytes` and `path_from_git` are UTF-8 on Windows.
   - Untracked symlink targets that are not valid Unicode would be
     converted lossily (they can never match a Git path).
   - Several tests are Unix-only.
2. **Repository-configured filters still run.** For working-tree
   comparisons, Git applies `filter.<driver>.clean` (e.g. Git LFS),
   `core.autocrlf` conversion, and `core.fsmonitor` hooks to compute the
   diff. No diff option disables them; this is the same class as Phase 1's
   fsmonitor note. External diff and textconv *are* disabled.
3. **Untracked binary detection** uses only the NUL heuristic.
   `.gitattributes` `binary`/`-diff` are not consulted for untracked files
   (tracked files follow Git's attributes).
4. **Combined-view conflict flag** comes from the snapshot used to build the
   engine. A conflict that starts later is not flagged until a new engine
   is built. Staged and Unstaged report `Unmerged` from Git directly.
5. **Staleness of working-tree files** is detected by `lstat` (size, mtime
   ns, inode, mode, type) plus Git's line counts. A rewrite that preserves
   all of these exactly would not be detected.
6. **Tracked `TooLarge`** reports `size: None`. Git output is cut off at the
   limit, so the full size is unknown.
7. **`load_all` holds the whole patch** (bounded by `max_total_patch_bytes`)
   and every `FileDiff` in memory, by design of an explicit "load all".
8. **Copies** are reported only when Git's `-C` detects them (source
   modified in the same diff). Git may pair empty files as copies or renames.
9. **Large files.** Git's histogram diff dominates their cost (about 0.5 s
   for 200k lines).
10. **Similarity pairing** is a documented heuristic. Some intuitively
    related short lines, such as `two` → `two changed`, are not paired.
11. **Submodules** are summarized (commits and dirty flag), not inspected.
    A working-tree commit is `None` when Git prints the zero ID.
12. **`DiffRowKind::Empty`** is part of the model but never produced; padding
    is expressed by `None` sides.

## 22. Deviations

1. **Plumbing instead of porcelain.** `diff-index --cached <base>`,
   `diff-files`, and `diff-index <base>` replace `git diff --cached`,
   `git diff`, and `git diff HEAD`, with the same semantics. Porcelain
   writes the index (§1); tests and a probe demonstrate it.
2. **Pinned base.** The base is the snapshot's HEAD commit ID (or the empty
   tree), not the symbolic `HEAD`. Discovery and loading therefore compare
   against the same commit even if HEAD moves.
3. **`--find-copies`** is added to the requested option set so copies can be
   reported.
4. **`DiffLine` field.** `DiffLine` has `ending: LineEnding` plus a
   `has_newline()` method instead of a `has_newline: bool` field, so CRLF
   can be represented.
5. **`DiffHunk` fields.** `DiffHunk` adds `section_heading` and
   `alignment: AlignmentQuality`.
6. **Combined view of conflicts.** In Combined, a conflicted file is shown as
   Git's real two-way diff and flagged `conflicted`, not as `Unmerged`,
   because Git does provide that diff.
7. **CLI.** The engine is exercised by a new `diffkit diff-stats [--load]`
   command instead of extending `diffkit inspect`. This keeps `inspect`'s
   cost and Phase 1 output unchanged.
8. **Phase 1 extensions:**
   - `RepositoryInfo::object_format` (one more flag on the existing
     `rev-parse`).
   - `git::commands` gains `output_limited`, `thread_invocations`,
     `path_bytes`, and the literal-pathspec environment.
9. **Synthesized hunks.** Type changes (file ↔ symlink), which Git prints as
   two sections, become one synthesized hunk. Untracked files get a
   synthesized `@@ -0,0 +1,N @@` header in Git's format.

## 23. Phase 2 acceptance checklist

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | All four comparison types work | PASS | `diff_comparisons::partial_staging_keeps_three_comparisons_distinct`, `diff_untracked::untracked_contents_as_additions`, `diff_cli::diff_stats_lists_every_comparison` |
| 2 | Staged and unstaged states remain distinct | PASS | `partial_staging_…` (A→B vs B→C, object IDs from `HEAD:f`/`:f`), `combined_is_not_the_sum_of_staged_and_unstaged` |
| 3 | Partial staging is correct | PASS | `partial_staging_keeps_three_comparisons_distinct`, `line_numbers_follow_each_side` |
| 4 | File discovery is separate from content loading | PASS | `discovery_carries_metadata_and_loading_is_per_file`; discovery returns `DiffFileList` only; `subprocess_counts` |
| 5 | Per-file lazy loading works | PASS | `subprocess_counts` (1 call per file, 0 for untracked), `load_all_matches_per_file_loads_and_reports_new_changes` |
| 6 | Unified diff parsing is robust | PASS | 20 `parser` unit tests incl. 12 malformed inputs; integration tests over real Git output, incl. hostile paths |
| 7 | Hunk ranges and line numbers are correct | PASS | `hunk::line_numbers_are_independent_per_side`, `zero_count_hunks`, `separated_hunks_and_context_size` (`-2,7 +2,7`, `-27,7`), `large_file_with_scattered_changes` (line 49,501), `line_numbers_follow_each_side` |
| 8 | Myers alignment is implemented and tested | PASS | `alignment.rs`; `myers_is_minimal_on_random_inputs` (2,000 cases vs oracle), `edit_cost_bound_is_explicit_and_deterministic`, `large_block_with_common_lines_completes` |
| 9 | Added, removed, modified, context rows are represented | PASS | `single_line_replacement_is_a_modified_row`, `insertion_only_and_deletion_only`, `staged_addition_and_deletion`, row-kind assertions throughout |
| 10 | Renames and deletions are handled | PASS | `rename_with_and_without_modification`, `renames_keep_both_unusual_paths`, `staged_addition_and_deletion`, `unstaged_deletion` |
| 11 | Binary files represented without text decoding | PASS | `binary_addition_modification_and_deletion` (no hunks, exact sizes), `binary::nul_within_first_8000_bytes_is_binary` |
| 12 | Untracked files handled without repository mutation | PASS | `reading_untracked_files_does_not_touch_the_repository`, `diff_engine_leaves_every_file_untouched` |
| 13 | Unusual paths are preserved | PASS (Unix) | `unusual_names_in_every_comparison` (13 names × 3 comparisons + `load_all`), `non_utf8_tracked_path`, `parser::quoted_paths_and_trailing_tab` |
| 13w | Path handling on Windows | NOT TESTED | Compiles and passes clippy for `x86_64-pc-windows-msvc`; not executed |
| 14 | Conflicts have an explicit representation | PASS | `conflicts_are_explicit_in_staged_and_unstaged` (`Unmerged`, no two-way diff), `combined_view_of_a_conflict_is_flagged` |
| 15 | Mode changes, symlinks, type changes, empty, no-EOL, CRLF | PASS | `mode_changes`, `symlink_target_change`, `file_replaced_by_symlink_is_a_type_change`, `empty_files`, `missing_final_newline`, `crlf_and_mixed_line_endings` |
| 16 | Changes between discovery and loading handled | PASS | `restaged_file_is_stale`, `unstaged_file_edited_or_deleted_after_discovery_is_stale`, `change_reverted_after_discovery_is_missing`, `vanished_and_changed_files_are_reported` |
| 17 | Safety limits are explicit; nothing truncated | PASS | `size_limit_is_explicit`, `patch_size_limits`, `limited_output_reports_overflow_instead_of_truncating` |
| 18 | No external diff/textconv, no shell, no mutation, no network | PASS | `external_diff_and_textconv_are_never_run` (with control), `diff_stats_runs_only_allowed_git_commands`, `only_the_command_layer_starts_processes`, fingerprint test |
| 19 | All required tests pass | PASS | `cargo test`: 236 passed, 0 failed |
| 20 | Phase 1 regression tests still pass | PASS | All 97 Phase 1 integration tests and 36 Phase 1 unit tests pass |
| 21 | All five Cargo validation commands pass | PASS | §18 |
| 22 | Performance measurements recorded | PASS | §15, `docs/PERFORMANCE_BUDGET.md`, `examples/bench_diff.rs` |
| 23 | Documentation updated | PASS | `PHASE_2_DIFF_ENGINE.md`, `DATA_MODEL.md`, `ARCHITECTURE.md`, `TESTING_STRATEGY.md`, `PERFORMANCE_BUDGET.md`, `ENGINEERING_RULES.md`, `TECH_STACK.md`, README |
| 24 | No Phase 3 functionality | PASS | No TUI, rendering, watcher, async, or new dependency; `diff-stats` prints counts only |
| 25 | Final report complete | PASS | This document |

Phase 2 is ready for review. No Phase 3 work will begin without approval.
