# Phase 3 — Terminal UI

Status: implemented (see `docs/PHASE_3_IMPLEMENTATION_REPORT.md`). Phase 4
(watching and automatic refresh) has not started.

## Scope

`diffkit` and `diffkit PATH` open an interactive Ratatui/Crossterm viewer of
the Phase 1 snapshot and the Phase 2 diff engine. Diagnostic subcommands
(`inspect`, `diff-stats`) are unchanged and non-interactive. Everything stays
read-only and offline.

## Modules (`src/tui/`, in the library)

| Module | Role |
|---|---|
| `app.rs` | `App` state and updates. The only TUI code that calls the engines: `App::open`, `load_pending`, `OpenSelected`, `refresh` |
| `actions.rs` | `Action` enum: what the user wants, independent of keys |
| `keymap.rs` | Crossterm key events → `Action` (press only; overlays capture keys) |
| `diff_view.rs` | `DiffView`: display rows built once per loaded diff (sanitized text, widths, hunk starts) |
| `layout.rs` | Pure geometry shared by state (scroll bounds) and rendering |
| `ui/` | Pure rendering from `&App`: `header`, `sidebar`, `diff_panel`, `status`, `overlays`, `theme` |
| `terminal.rs` | `TerminalGuard` (raw mode + alternate screen, RAII), panic hook, event loop, `run` |

Shared display helpers live in `src/display.rs` (`display_path`,
`sanitize_content`, `escape_text`, width-aware truncation).

## Data flow

```text
App::open(path)     snapshot::inspect + DiffEngine::from_snapshot + discover ×4   (6 Git calls, no diff)
event loop          draw(&App) → read event → apply(Action) … drain queued events
                    → load_pending()     at most one load_file_diff, for the selected file
                                         result → DiffView (or LoadFailure) → bounded cache (16)
Refresh (r)         re-inspect + rediscover all; selection kept by (old path, new path)
```

Rendering slices the cached `DiffView` rows for the visible window. It never
runs Git, parses, aligns, or re-sanitizes.

## Behavior

- **Startup:** opens on the first non-empty comparison, in the order
  unstaged, staged, untracked, combined.
- **Selection:** remembered per comparison. Switching comparison resets the
  diff scroll and loads that comparison's selected file.
- **Side-by-side rows:** each row is one Phase 2 `AlignedRow`.
  - Context rows show both sides.
  - Removed rows show only the old side; added rows only the new side.
  - Modified rows show both sides, marked `~`.
  - Missing sides are blank. No line number is ever invented.
- **Hunks:** `]`/`[` move between Phase 2 hunk starts. The selected hunk's
  header is marked `▶`.
- **Scrolling:**
  - Vertical scrolling is clamped to the content.
  - Horizontal scrolling moves both content columns together; the gutters
    (numbers and markers) stay fixed.
  - Long lines are never cut; scroll to see them.
- **Stale and vanished files** (Phase 2 errors) show a message and
  "press r to refresh". Outdated content is never shown.
- **Special content** (binary, submodule, unmerged, too large, unreadable,
  nested repository, no line changes) has its own explanation panel.
- **Layout:**
  - Below 40×10 a resize warning replaces the interface.
  - When the diff panel is narrower than two readable columns, it says so
    and suggests `b` (hide the sidebar) or `<`.
- **Terminal lifecycle:**
  - `TerminalGuard` restores the terminal on drop: normal exit, error
    return, and unwinding.
  - The panic hook restores the terminal before the panic message.
  - No cursor-position query is ever sent (it hangs on terminals that never
    answer).
  - Mouse capture is not enabled.

## Keys

See README or press `?`. The help text comes from `ui::overlays::KEYS`.

## Out of scope

Watching, automatic refresh, background work, staging and other Git
writes, history browsing, branch switching, fetching, conflict
resolution, editing, and syntax highlighting.
