# Phase 5 — Advanced Git Support, Hardening, and v1.0

Status: implemented (see `docs/PHASE_5_IMPLEMENTATION_REPORT.md`).
Acceptance and release are separate decisions; see
`docs/RELEASE_CHECKLIST.md`. This is the last planned development phase.

## Scope delivered

| Area | Where | How |
|---|---|---|
| Conflict stages | `git::status`, `git::models::ConflictStages`, `diff::ConflictInfo` | From porcelain v2 `u` records (already read); no extra Git process, no index access |
| Operations in progress | `git::state` | Existence and small contents of Git's per-worktree state files |
| Submodule state | `diff::engine::submodule_worktree` | `lstat` of the path and of its `.git`; never descends |
| Type changes, symlink labels | `tui::ui::diff_panel` | From modes Git reported |
| Commit history | `git::history`, `tui::app::HistoryState`, `tui::ui::history` | `git log` (bounded), `rev-parse --verify --end-of-options` |
| Commit diffs | `DiffEngine::discover_commit`, `DiffComparison::Commit` | `diff-tree -r <first parent or empty tree> <commit>`; Phase 2/3 loading and rendering |
| `diffkit show` | `app::show` | The same APIs as plain text |
| Consistency guard | `sync::reconcile::StateMarker`, scheduler | Retry when Git state moves; never publish an unsettled result |
| Signal-safe exit | `tui::terminal::Signals` | `signal-hook` flags → event loop → normal restore path |
| Release | `Cargo.toml`, docs | 1.0.0, `rust-version`, LTO profile, changelog, release notes, checklist |

## Deliberately excluded

- Fetching, or any remote access.
- Any Git write: staging, resolving, checking out, initializing
  submodules.
- A history graph, branch picker, search, or arbitrary ranges. History is
  the latest 200 commits on HEAD.
- Recursion into submodules or nested repositories.
- A self-updater, installer, background service, or published binaries.
- Phase 6 (there is none).

## Design notes

- **Rebase backends.** `git rebase` and `git rebase -i` leave identical
  state since Git 2.26; Git's own status calls both "interactive". Diffkit
  reports the sequencer ("merge") backend versus the apply backend, which
  Git's files do distinguish, rather than guessing.
- **Merges in history** are compared with their first parent, as
  `git show --first-parent` would. The commit bar says so.
- **Shallow boundary commits** are not diffed. Comparing them with the
  empty tree would show every file as added.
- **Unsettled reconciliations.** When Git changes the index, HEAD, or refs
  during all three attempts, the result is withheld, the last consistent
  state stays on screen, and a full run follows once Git goes quiet. The
  stress test shows such results are often truly mixed.
