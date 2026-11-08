# Architecture

Diffkit is a library crate (`src/lib.rs`, modules `git` and `diff`) plus a
binary (`src/main.rs`, module `app`). The library owns everything about Git;
the binary owns argument handling and presentation.

```text
src/
  main.rs            entry point: passes argv to app::cli and exits with its code
  lib.rs             exposes `git`, `diff`, `display`, `sync`, and `tui`
  display.rs         safe display text: path quoting, content sanitizing, truncation
  app/
    cli.rs           argument parsing, dispatch, stdout/stderr, exit codes
    report.rs        formats a RepositorySnapshot as the text report
    diff_stats.rs    `diffkit diff-stats [--load]` diagnostic (Phase 2)
    show.rs          `diffkit show <COMMIT>` (Phase 5)
  git/
    commands.rs      the only place a process is started
    errors.rs        GitError and Result
    models.rs        typed state shared by all layers
    repository.rs    repository discovery
    status.rs        porcelain v2 reading and byte-level parsing
    branches.rs      branch, HEAD, and upstream resolution
    commits.rs       ahead/behind tracking
    snapshot.rs      assembles one RepositorySnapshot
    state.rs         operations in progress, from Git's state files (Phase 5)
    history.rs       recent commits, commit metadata, revision resolution (Phase 5)
  diff/              Phase 2 diff engine
    engine.rs        DiffEngine: discover / load_file_diff / load_all, staleness checks
    commands.rs      argument arrays for diff-index, diff-files, ls-files, cat-file
    discovery.rs     --raw --numstat -z parsing and DiffFile construction
    parser.rs        byte-oriented unified diff parser
    hunk.rs          line numbering, line endings, hunk validation, untracked hunks
    alignment.rs     Myers (linear space, bounded) + line pairing → AlignedRow
    binary.rs        binary heuristic for untracked files, blob sizes
    untracked.rs     safe discovery and reading of untracked files
    models.rs        diff data model
    errors.rs        DiffError
  sync/              Phase 4 synchronization: watcher, classifier, debounce,
                     reconcile, scheduler (worker thread), diff cache
  tui/               Phase 3 terminal interface (Ratatui + Crossterm)
    app.rs           App state and updates; the only TUI code that calls the engines
    actions.rs       Action enum
    keymap.rs        key events → actions
    diff_view.rs     display rows built once per loaded diff
    layout.rs        pure screen geometry
    terminal.rs      terminal guard, panic hook, event loop
    ui/              rendering: header, sidebar, diff_panel, status, overlays, theme
```

## Layers and responsibilities

| Layer | Module | Responsibility |
|---|---|---|
| CLI | `app::cli` | Reads arguments, chooses the command, prints the report or the error, returns exit codes (0 ok, 1 inspection failed, 2 usage error). Contains no Git logic. |
| Presentation | `app::report` | Turns a snapshot into text. Escapes ambiguous paths for display only. |
| Git command | `git::commands` | Runs `git -C <dir> <args…>` with `std::process::Command`, discrete `OsStr` arguments, closed stdin, `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_LAZY_FETCH=1`. Maps spawn failures to `GitNotFound` / `GitNotExecutable` / `Io`, non-zero exits to `CommandFailed` with stderr. Converts Git path bytes to native paths. |
| Repository | `git::repository` | One `rev-parse` answering inside-work-tree, bare, shallow, Git dir, common dir, and toplevel. Classifies failures as not a repository, not a work tree, or a command failure. |
| Status | `git::status` | One `git status --porcelain=v2 -z --branch --ahead-behind --untracked-files=all`. Parses NUL-terminated records as bytes into `FileStatus` values and keeps the `# branch.*` headers raw. |
| Branch | `git::branches` | Interprets `branch.oid`/`branch.head` into `BranchInfo`; resolves the upstream (`UpstreamInfo`) using `branch.upstream`, `branch.ab`, and at most one `rev-parse`. |
| Commit | `git::commits` | Interprets `branch.ab` into `CommitTrackingInfo`, or records why counts are unavailable. |
| Snapshot | `git::snapshot` | Calls the layers in order and returns one `RepositorySnapshot`. |
| Model | `git::models` | Plain data types and predicates; no I/O and no formatting. |

## Data flow for `diffkit inspect`

```text
cli::run
  └─ snapshot::inspect_current_dir
       ├─ repository::discover_from(cwd)   git rev-parse …                 (1 call; 3 more only if a path contains '\n')
       ├─ status::read(root)               git status --porcelain=v2 …     (1 call)
       ├─ branches::resolve                from headers; git for-each-ref  (only when HEAD is unborn)
       ├─ branches::resolve_upstream       git rev-parse @{upstream} …     (only when an upstream ref exists, or HEAD is unborn)
       └─ commits::tracking                from the branch.ab header       (no call)
  └─ report::render(&snapshot) → stdout
```

A typical repository with an upstream costs three Git subprocesses; one
without an upstream costs two. The status call is made exactly once and its
headers feed the branch and commit layers.

The commands run sequentially, so a Git operation running concurrently in
another tool can land between them. Each piece is internally consistent;
the snapshot as a whole is not atomic.

## How Git commands are executed

`git::commands::output` is the single spawn site (verified by the
`only_the_command_layer_starts_processes` test). It never uses a shell,
never concatenates strings into a command line, and pins the working
directory with `-C`. `run` wraps it and turns a non-zero exit into
`GitError::CommandFailed { args, code, stderr }`. Callers that expect a
specific non-zero exit (for example `rev-parse --verify --quiet`, which
exits 1 for a missing ref) use `output` and interpret the code themselves.

Diffkit does not override the user's Git configuration. The two
environment variables it sets only enforce the read-only and offline
contract (see `ENGINEERING_RULES.md`).

## How models are shared

`git::models` types are public and free of display logic. The CLI consumes
them through `RepositorySnapshot`; integration tests assert on them
directly; later phases can call `git::snapshot::inspect` or the individual
layers. Missing information is represented by `Option` or a dedicated enum
variant, so callers cannot mistake "unavailable" for "zero".

## Phase 2: diff engine

```text
DiffEngine::from_snapshot(&snapshot)       HEAD, object format, conflicted paths (no Git call)
  discover(Staged|Unstaged|Combined)      git diff-index [--cached] <base> | git diff-files
                                           --raw --numstat -z → discovery::parse_raw_numstat
                                           → DiffFile list (+ lstat stamps for working-tree sides)
  discover(Untracked)                     git ls-files --others --exclude-standard -z (+ lstat)
  load_file_diff(list, id)                git diff-index/diff-files -p … -- <path(s)>   (bounded output)
                                           → parser::parse_patch → select section by paths
                                           → staleness checks → hunk::build_hunks
                                           → alignment::align per change block → FileDiff
                                           untracked: read file directly (no Git call)
  load_all(list)                          one patch command for the comparison, sections → files
```

`diff` depends on `git` (command layer, path conversion, models, snapshot);
`git` does not depend on `diff`. Parsing, numbering and alignment are pure
functions with no I/O, so they are unit-tested with byte fixtures. Only
`engine`, `discovery`, `binary`, and `untracked` touch Git or the file system.

The engine uses the plumbing commands `diff-index`/`diff-files` because
porcelain `git diff` rewrites `.git/index` after working-tree comparisons
even with `GIT_OPTIONAL_LOCKS=0` (see `docs/phases/PHASE_2_DIFF_ENGINE.md`).

## Phase 3: terminal interface

```text
main → app::cli::run
  []  or  [PATH]          tui::run(path)        needs a TTY; repository inspected before raw mode
  inspect / diff-stats    unchanged plain-text commands

tui::run
  App::open               snapshot::inspect → DiffEngine::from_snapshot → discover ×4 (no diff loaded)
  TerminalGuard::enter    raw mode, alternate screen, hidden cursor, panic hook
  loop:
    terminal.draw(ui::render(&app))           pure: reads App only
    event → keymap::action_for → app.apply    no Git (except Enter = reload, r = refresh)
    drain queued events, then app.load_pending()   ≤ 1 load_file_diff → DiffView → cache
  guard dropped           terminal restored (also on error or panic)
```

The interface lives in the library (`src/tui/`), so integration tests drive
`App` and render it through Ratatui's `TestBackend` without a terminal. The
binary only routes arguments. `display_path` moved from the binary's
`report.rs` to `src/display.rs` so the report and the interface share it;
`report.rs` re-exports it.

## Phase 4: synchronization

```text
                    ┌──────────── diffkit-sync worker thread ────────────┐
notify callback ──▶ │ bounded queue → classify → debounce → reconcile    │──┐
 (try_send)         │                         (Git: snapshot + lists)    │  │ SyncEvent
                    └────────────────────────────────────────────────────┘  │
diffkit-input thread ── terminal events ──────────────────────────────────▶ main loop channel (bounded)
                                                                            │
main thread: App::apply_sync → App::apply_reconciled (one-step swap) → load_pending → draw
```

- `sync` depends on `git` and `diff`, never on `tui`. `tui` consumes
  `sync`.
- Manual refresh (`r`) sends a request to the same worker. Without a
  worker (tests), it calls the same `reconcile` and `apply_reconciled`
  functions inline.
- `App::open` builds its first state with `reconcile`, and the worker
  starts from that state (`App::reconciled`).
- The diff cache moved from a 16-entry queue in `tui::app` to
  `sync::cache::DiffCache`, which is byte-budgeted and revalidated against
  Git's metadata after each reconciliation.

## Phase 5: advanced states, history, release

- `git::state` adds `RepositorySnapshot::operations` (merge, rebase, am,
  cherry-pick, revert, bisect) from the per-worktree Git directory. No Git
  process is involved.
- `git::status` keeps the conflict stages that porcelain v2 `u` records
  already contain (`FileStatus::stages`). `DiffEngine` attaches them, with
  the working-tree presence, to `DiffContent::Unmerged(ConflictInfo)`.
- `git::history` lists commits (`git log`) and resolves revisions
  (`rev-parse --verify --end-of-options`).
  `DiffEngine::discover_commit` lists a commit's files with
  `diff-tree -r <parent> <commit>` as `DiffComparison::Commit`. Loading,
  parsing, alignment, and display use the existing Phase 2 and 3 code,
  unchanged.
- `tui::App` gains `HistoryState` and `CommitView`. The history screen has
  its own selection, scroll, and 16 MiB diff cache. The working-tree
  state, cache, and synchronization are left untouched underneath.
  Rendering selects the history list or commit views in `ui::render`, and
  the diff panel draws whichever diff is on screen (`App::active_file`).
- `sync::reconcile` wraps each reconciliation in a `StateMarker` check and
  retries; unsettled results are withheld by the scheduler.
- `tui::terminal` routes SIGTERM, SIGHUP, and SIGINT to the event loop
  (Unix, `signal-hook`), so they take the normal restore path.

## Optimization pass (after Phase 5)

Supersedes the loop and reconciliation lines in the Phase 3 and 4 sections
above where they differ.

```text
reconcile(trigger, previous)
  rev-parse                       skipped when the trigger is local (worktree, index,
                                  HEAD, merge/rebase state): previous locations reused,
                                  rediscovered once if Git then fails
  StateMarker checked after the snapshot and after each list; an attempt stops as
  soon as Git state moved, pauses 10 ms × attempt, and starts over (last attempt full)
  status                          --no-ahead-behind on worktree/index/ignore triggers;
                                  upstream and counts reused if HEAD, branch, and
                                  upstream name are unchanged (else a full status)
  DiffEngine::discover_reported   tracked lists: diff-index/diff-files -- <tracked paths
                                  status reported>; none → no Git call; > 16 KiB of paths
                                  → whole tree. Untracked list: status's `?` records.

tui event loop
  app.take_load() → Loader thread (one job running, at most one waiting; a newer
  job replaces the waiting one) → LoadJob::run (Git + DiffView) → LoopEvent::Loaded
  → app.finish_load (dropped if the file's metadata changed meanwhile; reloaded if
  still on screen). App::load_pending runs the same three steps inline (tests).
```

- `Reconciled::snapshot` is an `Arc<RepositorySnapshot>`, so the worker
  keeps its base result and the interface keeps its state without copying
  every status entry.
- `hunk::build_hunks` consumes the parsed hunks and moves line text instead
  of copying it.
- `DiffEngine::load_each` hands each loaded file to a callback in list
  order and frees its patch text; `load_all` collects from it, and
  `diffkit diff-stats --load` summarizes from it without keeping the diffs.
- Line pairing computes each line's sorted bigrams once per change block
  and skips pairs whose bigram counts alone cannot reach the threshold.
- `DiffCache` keeps a recency index, so eviction does not scan entries.
