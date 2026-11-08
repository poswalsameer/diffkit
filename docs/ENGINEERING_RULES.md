# Engineering Rules

## Read-only guarantee

Diffkit only inspects. It must never stage, unstage, commit, amend, push,
pull, fetch, reset, restore, check out, switch, merge, rebase, cherry-pick,
modify remotes or upstream configuration, edit `.git/config`, write refs or
tags, create or delete branches, or intentionally modify `.git/index`.

`diffkit inspect` runs exactly three Git subcommands: `rev-parse`, `status`,
and `for-each-ref`. The Phase 2 diff engine adds four read-only ones:
`diff-index`, `diff-files`, `ls-files`, and `cat-file`. Phase 4
synchronization adds `check-ignore`, the only command run without
`GIT_LITERAL_PATHSPECS` (it rejects that variable). Its paths are prefixed
with `./`, so pathspec magic cannot apply. Phase 5 history inspection adds
`log` (with `--no-show-signature --no-notes --no-mailmap --no-color` and an
explicit format) and `diff-tree -r <parent> <commit>` (with the same
`--no-ext-diff --no-textconv` options as the other diff commands).
When a reconciliation is triggered only by working-tree, index, or ignore
rule changes, `status` runs with `--no-ahead-behind` instead of
`--ahead-behind` and the previous counts are reused (they are recomputed
whenever HEAD, the branch, or the upstream name differs).
User-supplied revisions go through `rev-parse --verify --end-of-options`,
so a revision starting with `-` is never read as an option. Porcelain `git diff`
is deliberately not used, because it rewrites `.git/index` after
working-tree comparisons even with `GIT_OPTIONAL_LOCKS=0`. Every invocation
sets:

- `GIT_OPTIONAL_LOCKS=0` — `git status` would otherwise take the index lock
  and write back refreshed stat information. Tests prove that plain
  `git status` rewrites the index in our fixture and Diffkit does not.
- `GIT_NO_LAZY_FETCH=1` — in a partial clone, `git status` can fetch missing
  blobs from the promisor remote during rename detection. With this
  variable Git refuses instead, and Diffkit reports Git's error. This needs
  Git 2.45+.
- `GIT_LITERAL_PATHSPECS=1`, with `GIT_GLOB_PATHSPECS`,
  `GIT_NOGLOB_PATHSPECS`, `GIT_ICASE_PATHSPECS`, and `GIT_DIFF_OPTS`
  removed. Filenames passed after `--` are always literal paths.

Diff commands also pass `--no-ext-diff --no-textconv --no-color`, so
configured external diff programs and textconv filters never run
(`tests/diff_read_only.rs` proves it with a positive control).

Adding any other Git subcommand requires review, and the allow-lists in
`tests/read_only.rs`, `tests/diff_read_only.rs`, `tests/history.rs`
(`diffkit show`), and `tests/tui_cli.rs` (the running viewer) must be
updated deliberately.

## Git CLI is the source of truth

Do not read `.git/HEAD`, refs, or the index directly; ask Git. Do not
re-implement `.gitignore`, worktree, or ref resolution.

Two narrow exceptions, both in the per-worktree Git directory that
`rev-parse --absolute-git-dir` reports:

- **Operations in progress** (`git::state`). Git offers no machine-readable
  command for "a rebase is in progress". Porcelain v2 status omits it, and
  the long format is localized. Diffkit checks for the same state files
  `git status` and Git's own `git-prompt.sh` check: `MERGE_HEAD`,
  `rebase-merge/`, `rebase-apply/`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
  `sequencer/todo`, and `BISECT_LOG`. It reads only a few small values
  from them (step numbers, branch name, onto commit, bisect start), capped
  at 4 KiB each.
- **The reconciliation consistency guard** (`sync::reconcile`). It records
  the metadata (size, mtime, inode) of the index, the current branch's
  ref, `packed-refs`, `reftable/tables.list`, and the operation state
  files, plus the content of `HEAD`. It compares them before and after
  reading. These values decide only whether to read again; nothing shown
  comes from them.

## No shell, no interpolation

- Processes are started only in `git::commands`, with `Command::new("git")`.
- Arguments are passed individually as `OsStr`; nothing is concatenated into
  a command line. Paths are passed with `-C <dir>`.
- No `sh`, `bash`, `cmd`, or PowerShell, no hooks, no external diff
  programs, no aliases. `tests/read_only.rs` enforces the spawn rule.

Known residual: `git status` honours the user's configuration. If
`core.fsmonitor` names a hook script, Git runs it, exactly as it would for
the user's own `git status`. Diffkit does not override this (see the
Phase 1 report, Known Limitations).

## Dependencies

The Git and diff engines use only the Rust standard library. The Phase 3
interface uses `ratatui`, `crossterm`, and `unicode-width` (see
`TECH_STACK.md`). Adding any other dependency requires explicit approval
stating its name, purpose, why std is insufficient, its cost, and the
feature that needs it.

## Commit inspection (Phase 5)

- History is local and bounded: at most 200 commits from HEAD in one
  `git log`, whose output is capped at 32 MiB.
- Commit diffs reuse the Phase 2 discovery, parser, alignment, and
  staleness checks through `DiffComparison::Commit`. A merge is compared
  with its first parent only. A shallow-boundary commit is not diffed
  against the empty tree, because that would show every file as added.
- Commit messages, names, e-mails, and revisions typed by the user are
  untrusted text: escape them before display, including in error messages.

## Synchronization (Phase 4)

- Filesystem events are hints. Published state always comes from Git
  through the Phase 1 and Phase 2 APIs; never from accumulated events.
- Queues are bounded. Pending changes are a fixed-size bit set. One
  reconciliation runs at a time, on one worker thread per repository.
  There is no polling and no Git process per event.
- The worker owns the watcher. The interface thread only applies finished
  results, all at once.
- A result read while Git was changing the index, HEAD, or refs is not
  published (`Reconciled::settled`). Prefer showing the last consistent
  state over a mixed one.
- Repository-configured helpers can still run inside Git while state is
  read: clean/smudge filters (for example Git LFS) and `core.fsmonitor`
  hooks. External diff and textconv are disabled. Do not claim more.

## Terminal interface (Phase 3)

- Only `tui::app` calls the engines. Rendering (`tui::ui`) reads state and
  never runs Git, reads files, parses, or aligns.
- Everything shown is escaped through `display`. No byte from a filename or
  file content reaches the terminal as a control sequence.
- The terminal is always restored (RAII guard plus panic hook). Logs are
  never printed while the interface is active.

## Error handling

- Expected repository states — no upstream, missing tracking ref, detached
  HEAD, unborn branch, empty repository — are model values, not errors.
- Environmental failures (Git missing, not executable, path inaccessible,
  not a repository, not a work tree), Git failures (with exit code and
  stderr), and malformed output are distinct `GitError` variants.
- Never panic on repository state. Never turn a failure into an empty
  result. Keep Git's stderr in error messages.
- The CLI prints `diffkit: error: …` to stderr and exits 1, or exits 2 for
  usage errors. A closed stdout (`diffkit inspect | head`) is not an error.

## Testing

Every layer has unit tests. Behaviour is verified with integration tests
against real Git in isolated temporary repositories, using local bare
repositories for remote scenarios. Tests never use the network, the
developer's Git configuration, or a real repository. Never weaken or delete
a test to make it pass. See `TESTING_STRATEGY.md`.

## Separation of concerns

The CLI contains no Git parsing; the Git layer contains no formatting. Models
are plain data. New features extend the layer that owns the concern.

## Paths

Paths stay as `PathBuf`/`OsStr` internally. On Unix, Git's bytes map
directly to `OsStr`. On Windows, Git for Windows emits UTF-8, which is
decoded strictly. Anything undecodable is an error, never a lossy
substitution. Display escaping lives only in `app::report`.

## Performance

Do not read file contents, walk the working tree, compute diffs, load full
history, start threads, or issue redundant Git calls. Reuse a command's
output for every field it provides (the status headers supply branch, HEAD,
upstream name, and ahead/behind). Measure before claiming a target is met;
see `PERFORMANCE_BUDGET.md`.
