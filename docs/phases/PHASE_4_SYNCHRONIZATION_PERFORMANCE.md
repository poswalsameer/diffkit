# Phase 4 — Synchronization and Performance

Status: implemented (see `docs/PHASE_4_IMPLEMENTATION_REPORT.md`). Phase 5
has not started.

## Pipeline

```text
notify (FSEvents / inotify / ReadDirectoryChangesW)
  └─ handler: try_send into a bounded queue (4,096); when full, count a drop
sync worker thread (one per open repository)
  ├─ classify(path)       → ChangeSet bits, or irrelevant
  ├─ debounce             quiet 200 ms after the last event, at most 1 s after the first
  ├─ reconcile            Phase 1 snapshot + Phase 2 discovery, with safe list reuse
  ├─ same as before?      → Unchanged (no interface update)
  └─ publish SyncEvent    → interface channel → App::apply_sync → App::apply_reconciled
```

Watcher events never describe the result. Every published state comes from
Git.

## Modules (`src/sync/`)

| Module | Responsibility |
|---|---|
| `events.rs` | `ChangeSet` (fixed-size bit set of categories) and `SyncDiagnostics` counters |
| `classifier.rs` | `WatchLayout` (root, Git directory, common directory, excluded directories) and `classify` |
| `debounce.rs` | Deterministic quiet-period and maximum-delay debouncer (time injected) |
| `reconcile.rs` | `reconcile` (fresh snapshot + lists, with reuse rules), `Reconciled`, `ignored_directories` |
| `watcher.rs` | What to watch, the non-blocking handler, and watcher start-up |
| `scheduler.rs` | Worker thread: queue, classification, debounce, single reconciliation at a time, watcher restarts, `SyncService` handle and shutdown |
| `cache.rs` | `DiffCache`: byte-budgeted LRU with metadata revalidation and statistics |

## What is watched

| Repository layout | Watches |
|---|---|
| Standard (`.git/` inside the working tree) | The working tree, recursively. Git internals are classified, not separately watched. |
| Linked worktree (`.git` is a file), or a Git directory outside the tree | The working tree recursively; the per-worktree Git directory non-recursively (HEAD, index, merge and rebase state); the common directory non-recursively (packed-refs, config); the common `refs/` and `info/` recursively. |

Symlinks are not followed. Objects, logs, and hooks are never watched on
their own.

## Classification

| Path | Category |
|---|---|
| `index`, `index.lock`, `sharedindex.*` | INDEX |
| `HEAD` (this worktree's) | HEAD |
| `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE`, `MERGE_RR`, `AUTO_MERGE`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `sequencer/` | MERGE_STATE |
| `rebase-merge/`, `rebase-apply/`, `REBASE_HEAD` | REBASE_STATE |
| `refs/**`, `packed-refs`, `shallow` | REFS |
| `config`, `config.worktree` | CONFIG |
| `info/exclude`, `.gitignore` | WORKTREE + IGNORE_RULES |
| `modules/*/{HEAD,index,refs,packed-refs}` | WORKTREE (submodule state) |
| The working tree root, `.git` file or directory, a Git directory itself | STRUCTURE |
| Other working-tree paths | WORKTREE |
| Objects, logs, hooks, `FETCH_HEAD`, `ORIG_HEAD`, other worktrees' state, nested repositories' `.git`, ignored directories, reads | irrelevant |
| `Flag::Rescan`, watcher errors, dropped events | OVERFLOW |

## Reconciliation strategy

- Always a fresh snapshot: rev-parse and status.
- Comparison lists are rediscovered unless reuse is provably safe:
  - HEAD commit unchanged, and the previous list was valid;
  - Staged is reused for WORKTREE, IGNORE_RULES, REFS, or CONFIG triggers;
  - the other lists are reused only for REFS or CONFIG triggers.
- MANUAL, OVERFLOW, STRUCTURE, STARTUP, INDEX, and HEAD always rediscover.
- A result equal to the previous state is reported as `Unchanged`, and the
  interface is not touched.
- Ignored-directory exclusions are recomputed after INDEX, IGNORE_RULES,
  STRUCTURE, OVERFLOW, or MANUAL triggers: `ls-files --others --ignored
  --directory` plus `check-ignore --no-index`, keeping only directories
  that are themselves ignored.

## Concurrency

- One worker thread runs reconciliations sequentially, so they cannot
  overlap.
- Events that arrive during a run are drained before its result is
  published; they mark it `pending` and start the next window.
- The interface thread blocks on one channel carrying input (from an input
  thread) and sync results. It is never blocked by a reconciliation.
- Shutdown order: stop and join the input thread, drop the channel, stop
  the worker with a 2 s bounded wait (detached afterwards), restore the
  terminal.

## Failure handling

- **Watcher start failure:** "Watcher unavailable", manual refresh still
  works. Up to 3 automatic restarts with 1 s, 5 s, and 30 s backoff; a
  manual refresh retries again.
- **Watcher errors and dropped events:** OVERFLOW, which forces a full
  reconciliation and a watcher rebuild.
- **Reconciliation failure** (for example, the repository was removed):
  "! Refresh required". The previous state is kept. There is no automatic
  retry loop; the next event or `r` retries.
- There is no polling fallback.

## Cache

`DiffCache`: LRU by recency, with a 64 MiB approximate byte budget
(`DiffView::approx_bytes`). The diff on screen is protected from eviction.
After every reconciliation, an entry survives only if Git's fresh
`DiffFile` metadata for the same key (comparison, old path, new path) is
identical: object IDs, working-tree stamp, line counts, modes, kind,
binary, and conflict. Statistics: hits, misses, evictions, invalidations,
bytes, and peak bytes.

## Configuration

`diffkit --no-watch [PATH]` disables watching. `SyncConfig` (library)
holds the quiet period, maximum delay, queue capacity, watch flag, and
restart limit.

## Out of scope

Staging and other writes, fetching, remote synchronization, a daemon,
persistent storage, history browsing, conflict resolution, and Phase 5
features.
