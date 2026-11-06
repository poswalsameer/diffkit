# Phase 2 — Diff Engine

Status: implemented (see `docs/PHASE_2_IMPLEMENTATION_REPORT.md`). Phase 3
(terminal UI) has not started.

## Scope

A reusable library (`src/diff/`) that discovers changed files per comparison,
loads per-file diffs lazily, parses Git's unified diff output into byte-exact
structured lines, and aligns changed lines into rows ready for a future
side-by-side view. It never prints; `diffkit diff-stats` is a diagnostic
only.

## Comparison semantics

| `DiffComparison` | Meaning | Git command |
|---|---|---|
| `Staged` | HEAD → index | `git diff-index --cached <base>` |
| `Unstaged` | index → working tree | `git diff-files` |
| `Combined` | HEAD → working tree (tracked files) | `git diff-index <base>` |
| `Untracked` | nothing → untracked file | `git ls-files --others --exclude-standard -z`, then read the file |

`<base>` is the HEAD commit from the Phase 1 snapshot, or the empty tree
(SHA-1 `4b825dc…` / SHA-256 `6ef19b4…`) when HEAD is unborn.

The three tracked comparisons are separate views. For HEAD = A,
index = B, working tree = C, the engine reports Staged A→B, Unstaged B→C,
and Combined A→C. Combined is not the sum of the other two: a line staged
and then removed again in the working tree appears in Staged and Unstaged
but not in Combined (`combined_is_not_the_sum_of_staged_and_unstaged`).

**Why plumbing commands.** Porcelain `git diff` refreshes and *rewrites*
`.git/index` after a working-tree comparison, even with
`GIT_OPTIONAL_LOCKS=0` or `--no-optional-locks` (verified with Git 2.54). The
plumbing commands `diff-index` and `diff-files` never write. They do list
files whose stat data changed but whose content did not. Such a file has a
`--raw` record but no `--numstat` record, so discovery drops it and counts it
in `DiffFileList::stat_only_entries`.

## API

```rust
let engine = DiffEngine::open(path, DiffOptions::default())?;   // or from_snapshot(&snapshot, opts)
let list   = engine.discover(DiffComparison::Staged)?;          // metadata only, 1 Git call
let diff   = engine.load_file_diff(&list, list.files[0].id)?;   // 1 Git call for that file
let all    = engine.load_all(&list)?;                           // 1 Git call for the whole list
for hunk in diff.hunks() { for row in &hunk.rows { /* row.kind, row.old_line, row.new_line */ } }
```

- `DiffFileList { comparison, base, files: Vec<DiffFile>, stat_only_entries }`
- `DiffFile`: id, comparison, old/new path, `FileChangeKind`, old/new mode,
  similarity, old/new object ID (only what Git printed; zero IDs are `None`),
  `BinaryStatus`, `LineStats` from `--numstat`, `conflicted`, working-tree
  stamp. `has_hunks()` tells whether loading will produce hunks.
- `FileDiff { file, content: DiffContent, source, old_oid, new_oid }`, where
  `DiffContent` is `Text { hunks }`, `Binary(BinarySummary)`,
  `Submodule(SubmoduleChange)`, `Unmerged`, `TooLarge { limit, size }`,
  `Unreadable { reason }`, or `NestedRepository`.
- `DiffHunk { old_start, old_count, new_start, new_count, header,
  section_heading, rows: Vec<AlignedRow>, alignment }`.
- `AlignedRow { kind: Context | Added | Removed | Modified | Empty,
  old_line: Option<DiffLine>, new_line: Option<DiffLine> }`. A missing side
  is `None`, never an empty line. `Empty` is reserved for presentation
  padding; the engine never emits it.
- `DiffLine { content: Vec<u8>, line_number, ending: Lf | CrLf | None }`.
- Errors: `DiffError::{Git, Parse { line, detail }, UnknownFile, Stale { path,
  reason }, Vanished, OutsideRepository, Io}`.

## Git options

Every `diff-index`/`diff-files` call passes `--no-ext-diff --no-textconv
--no-color --no-relative --find-renames --find-copies
--diff-algorithm=histogram --submodule=short`. Discovery adds `--raw
--numstat -z --no-abbrev`. Patches add `--patch --full-index --src-prefix=a/
--dst-prefix=b/ --inter-hunk-context=0 -U<context>`. Paths go after `--`.
The command layer sets `GIT_LITERAL_PATHSPECS=1` (and removes the glob/icase
variants and `GIT_DIFF_OPTS`), so a file named `:(attr:x)y` or `*` is matched
literally.

## Discovery and lazy loading

Discovery runs one command per comparison and never reads file content into
Diffkit. `--raw` supplies modes, object IDs, status, and rename/copy pairs.
`--numstat` supplies line counts or binary. Records are paired by path.
For a working-tree side, an `lstat` stamp is recorded (size, mtime, type,
inode, mode).

`load_file_diff` runs one `git diff-files`/`diff-index -p` limited to that
file's path(s). Both paths are passed for renames and copies so Git can pair
them. The output is parsed and the section whose resolved paths match is
selected. Before and after the Git call, and against the patch itself, the
engine checks:

- the working-tree stamp is unchanged;
- object IDs from the patch's `index` line match discovery, on repository
  sides;
- binary classification and `+/-` line counts match discovery;
- Git still reports the change.

Any mismatch is `DiffError::Stale` with a `StaleReason`. New content is never
attached to old metadata.

`load_all` runs one patch command for the whole comparison and distributes
sections to files. Sections Git reports that are not in the list become
`DiffCollection::unexpected_paths`. If the total output would exceed
`max_total_patch_bytes`, it falls back to per-file loads.

There is no cache, no background work, and no persistent state.

## Unified diff parser (`parser.rs`)

The parser is byte-oriented and splits on `\n` only. It understands:
`diff --git`, `---`/`+++` (including `/dev/null`, C-quoted paths, and the tab
Git appends to names containing a space, also after a closing quote), `old
mode`/`new mode`, `new file mode`/`deleted file mode`, `similarity
index`/`dissimilarity index`, `rename from/to`, `copy from/to`, `index
<old>..<new>[ <mode>]`, `Binary files … differ`, hunks, and `\ No newline at
end of file`.

Combined (`diff --cc`) sections and `* Unmerged path` notices are recognized
and skipped as unmerged. Unknown header lines are kept in
`extended_headers`.

Hunk bodies are read by count. A missing count means 1. A hunk shorter or
longer than its header, a bad header, overlapping hunks, or a body line with
an unknown prefix is a `DiffError::Parse` with the line number. Paths are
resolved in priority order: rename/copy lines, then `---`/`+++`, then the
`diff --git` header. The header is only trusted when it is unambiguous
(quoted, or both names equal).

## Myers alignment (`alignment.rs`)

Git's histogram algorithm determines hunk boundaries. Inside each hunk,
context lines are fixed, and each run of removed and added lines is aligned
with Myers' algorithm, in the linear-space "middle snake"
divide-and-conquer form. Cost is O((N+M)·D) time and O(N+M) memory, with no
N×M matrix.

- Lines are compared by content and line ending, interned to integers.
- If the removed and added runs share no line, the edit script is
  delete-then-insert without searching. That is already the shortest.
- **Bound:** each middle-snake search stops after
  `AlignmentOptions::max_edit_cost / 2 + 1` diagonals (default cost 4096).
  A sub-range beyond that is emitted as delete-then-insert, and the hunk is
  marked `AlignmentQuality::Bounded`. The bound depends only on the input,
  so the result is deterministic.
- **Pairing policy**, for each run of deletions and insertions between equal
  lines: walk the deleted lines in order. For each, consider the next
  `pairing_window` (16) unpaired inserted lines. Pick the most similar one,
  with ties going to the earliest. Pair them as `Modified` if the
  similarity is at least `pairing_threshold` (40%). Similarity is the
  Sørensen–Dice coefficient of byte bigrams of the trimmed lines, using the
  first 512 bytes. Identical trimmed lines score 100. Single differing
  characters score 0. Skipped inserted lines become `Added`; unpaired
  deleted lines become `Removed`. Both columns stay in order.

Example: `const age = 20;` → `const age = 21;` between two unchanged lines
gives `Context, Modified, Context`.

## Binary files

Binary status comes from `--numstat` (`-`/`-`) and the patch's `Binary files
… differ` line. Content is never decoded or placed in line models.
`BinarySummary` sizes come from `git cat-file -s` for repository objects and
from the file size on disk for a working-tree side. Untracked files use
Git's own heuristic: a NUL byte in the first 8000 bytes.

## Untracked files

Discovered with `ls-files --others --exclude-standard -z`, which applies
every ignore source. Each file is read directly; nothing is staged or
hashed.

- Symlinks are not followed: their target is the content.
- Paths must have only normal components, and the parent directory must
  resolve inside the repository.
- The opened file must be the one that was `lstat`ed, and its metadata must
  be unchanged after reading. Otherwise the result is `Stale`; a missing
  file is `Vanished`.
- Files above `max_untracked_bytes` (default 8 MiB) are `TooLarge` and are
  not read. Permission errors give `Unreadable`. Directories that are
  repositories give `NestedRepository`.
- Content becomes one `@@ -0,0 +1,N @@` hunk of `Added` rows.

## Special cases

- Renames and copies keep both paths and Git's similarity.
- Mode-only changes are `ModeChanged` with no hunks.
- Type changes (for example file → symlink), which Git prints as a deletion
  plus an addition, become one `TypeChanged` file with one aligned hunk.
- Symlinks are text whose single line has no newline.
- Empty files have no hunks.
- CRLF, LF, and a missing final newline are kept per line.
- Unmerged paths are `FileChangeKind::Unmerged` with `DiffContent::Unmerged`
  in Staged and Unstaged; no combined diff is presented as two-way. In
  Combined, Git does produce a real HEAD → working-tree diff of the file with
  conflict markers; it is shown and flagged `conflicted`.
- Submodules are `DiffContent::Submodule` with old and new commits and
  `new_dirty`.

## Limits

`DiffOptions` defaults:

| Option | Default |
|---|---|
| `context_lines` | 3 |
| `max_file_patch_bytes` | 64 MiB |
| `max_total_patch_bytes` | 512 MiB |
| `max_untracked_bytes` | 8 MiB |
| `alignment.max_edit_cost` | 4096 |
| `alignment.pairing_threshold` | 40% |
| `alignment.pairing_window` | 16 |

Git output beyond a limit is not truncated. The process is killed and the
file is `TooLarge`.

## Out of scope (later phases)

Terminal UI, rendering, syntax highlighting, watching, refresh, caching,
conflict resolution, commit browsing, and anything networked.

## Acceptance criteria

All four comparisons work and stay distinct. Partial staging is correct.
Discovery is separate from loading. Lazy per-file loading works. Parsing is
robust. Hunk ranges and line numbers are correct. Myers alignment is
implemented and tested. Context, added, removed, and modified rows are
produced. Renames and deletions are handled. Binary files are never
decoded. Untracked files are handled without mutation. Unusual paths are
preserved. Conflicts are explicit. All tests pass, including Phase 1. All
five Cargo commands pass. Performance is measured. Documentation is updated.
