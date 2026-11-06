# Engineering Rules

## Read-only guarantee

Diffkit only inspects. It must never stage, unstage, commit, amend, push,
pull, fetch, reset, restore, check out, switch, merge, rebase, cherry-pick,
modify remotes or upstream configuration, edit `.git/config`, write refs or
tags, create or delete branches, or intentionally modify `.git/index`.

`diffkit inspect` runs exactly three Git subcommands: `rev-parse`, `status`,
and `for-each-ref`. The Phase 2 diff engine adds four read-only ones:
`diff-index`, `diff-files`, `ls-files`, and `cat-file`. Porcelain `git diff`
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
`tests/read_only.rs` and `tests/diff_read_only.rs` must be updated
deliberately.

## Git CLI is the source of truth

Do not read `.git/HEAD`, refs, or the index directly; ask Git. Do not
re-implement `.gitignore`, worktree, or ref resolution.

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
