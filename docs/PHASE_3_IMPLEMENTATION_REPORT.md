# DIFFKIT — PHASE 3 IMPLEMENTATION REPORT

Date: 2026-11-06. Host: macOS 26.5 (arm64), Apple M5, Git 2.54.0, rustc/cargo
1.98.1. Baseline: Phase 2 commit `4cf8ca0`. Nothing committed or pushed.

## 1. Executive summary

`diffkit` (or `diffkit PATH`) now opens an interactive Ratatui/Crossterm
viewer. It has:
- A header with repository and branch information and a comparison tab bar.
- A scrollable sidebar of changed files.
- A side-by-side diff panel with independent line numbers, change markers,
  hunk navigation, and horizontal scrolling.
- A status line, a key-hint footer, and help and error overlays.

All four Phase 2 comparisons are available, and partial staging stays
distinct. Special content (binary, conflict, submodule, mode change,
rename, empty, too large, unreadable, nested repository) has explicit
panels. Stale or vanished files are reported, and old content is never
shown for them. Every displayed path and content byte is escaped, so
nothing from the repository can reach the terminal as a control sequence.

Loading is lazy: startup lists files (6 Git calls) and loads exactly one
diff. Rendering, scrolling, and resizing make 0 Git calls; this is asserted
in tests with the per-thread invocation counter. A bounded 16-entry cache
makes returning to a file free. The terminal is restored by an RAII guard
and a panic hook. This was verified in real pseudo-terminal sessions,
which also exposed and fixed a hang-prone cursor-position query.

`inspect` and `diff-stats` are unchanged. The Phase 1 and Phase 2 engines
(`src/git/`, `src/diff/`) were not modified.

**Results:** 287 tests (102 unit, 185 integration), 0 failed and 0
ignored. 51 tests are new (15 unit, 36 integration). All five Cargo
commands pass with no warnings.

**Status:** the Definition of Done is met on macOS (§26 of this report's
checklist). Windows compiles and lints but was not run. Human-operated
interactive testing in a terminal emulator was not performed by me; see
§15.

## 2. Architecture overview

```text
main.rs → app::cli::run (binary: argument routing only)
  []  / [PATH]           → diffkit::tui::run(path)
  inspect / diff-stats   → unchanged plain-text commands

diffkit::tui (library)
  App::open          snapshot::inspect + DiffEngine::from_snapshot + discover ×4      [engines]
  TerminalGuard      raw mode + alternate screen + hidden cursor; restores on drop/panic
  event loop         draw(ui::render(&App)) → keymap → App::apply → drain → App::load_pending
  App::load_pending  ≤ 1 DiffEngine::load_file_diff → DiffView::new → bounded cache   [engines]
  ui::*              pure rendering from &App                                        [no Git]
```

Separation is strict:
- Only `tui::app` calls the engines.
- `tui::ui` only reads state.
- `tui::layout` is pure geometry, used both for scroll bounds and for
  drawing, so they cannot disagree.

## 3. Module structure

| File | Lines (approx.) | Responsibility |
|---|---:|---|
| `src/tui/mod.rs` | 20 | Module map, re-exports `App`, `Action`, `run` |
| `src/tui/app.rs` | 560 | State: snapshot, engine, per-comparison file lists and selection, cache, scroll, sidebar, overlay, status, size; `open`, `apply`, `dispatch`, `load_pending`, `refresh` |
| `src/tui/actions.rs` | 40 | `Action` enum |
| `src/tui/keymap.rs` | 170 | Key → action, with tests |
| `src/tui/diff_view.rs` | 200 | `DiffView`, `ViewRow`, `Cell`, `Notice`: display rows built once per load |
| `src/tui/layout.rs` | 190 | Screen and diff-panel geometry, with tests |
| `src/tui/terminal.rs` | 190 | `TerminalGuard`, `restore_terminal`, `install_panic_hook`, `run`, event loop, `TuiError` |
| `src/tui/ui/{mod,header,sidebar,diff_panel,status,overlays,theme}.rs` | 900 | Rendering |
| `src/display.rs` | 260 | `display_path` (moved from the binary), `sanitize_content`, `escape_text`, width-aware truncation, with tests |

The suggested `src/app/state.rs`, `src/ui/`, and `src/terminal/` layout
was adapted to `src/tui/{app,…,ui,terminal}` inside the library. The
binary's existing `app` module (`cli`, `report`, `diff_stats`) keeps its
role, and integration tests can drive the interface without a terminal.

## 4. Dependency additions

| Crate | Version / features | Why |
|---|---|---|
| `ratatui` | 0.30.2, `default-features = false`, `crossterm_0_29` + `layout-cache` | Required TUI framework. Calendar widget and macros excluded. |
| `crossterm` | 0.29.0 | Required backend: raw mode, alternate screen, events. Same version as Ratatui's backend (`cargo tree -d` shows one copy). |
| `unicode-width` | 0.2.3 | Character display widths for correct truncation and tab expansion with wide characters. Already in Ratatui's tree at this version. |

Not added: an async runtime, Git library, database, network client,
syntax highlighter, or logging crate. Release binary: 1,290,368 bytes.

## 5. Application state design

`App` holds:
- The `RepositorySnapshot` and the `DiffEngine` built from it.
- The active `DiffComparison`.
- One `ModeState { list: Result<DiffFileList, String>, selected, offset }`
  per comparison. Selection and sidebar scroll are remembered per
  comparison; a failed discovery is shown, not hidden.
- A bounded cache: `VecDeque<((DiffComparison, FileKey), Loaded)>`, 16
  entries, oldest evicted first. `Loaded` is `View(Box<DiffView>)` or
  `Failed(LoadFailure { kind: Stale | Vanished | Other, message })`.
- `pending_load`.
- `DiffScroll { top, left, hunk }`.
- `Sidebar { width, visible }`.
- `Option<Overlay>` (Help or Error) and `Option<Status>` (Info, Warning,
  Error).
- The terminal size and the quit flag.

File identity is `FileKey = (old path, new path)`, the native `PathBuf`s
from Phase 2. Display strings are never used as identity. Refresh restores
each comparison's selection by key, then by path, then by clamped index.

`apply(Action)` never runs Git, except `OpenSelected` (Enter reloads the
file) and `Refresh`. Selection changes only set `pending_load`.
`load_pending()` runs at most one `load_file_diff`. The event loop calls it
after handling every queued key, so holding `j` does not load each file it
passes.

## 6. Rendering architecture

`ui::render(frame, &App)` lays out the screen with `layout::screen`. Below
40×10 it draws a resize warning only. The regions are:
- **Header:** brand, repository folder, worktree and shallow markers,
  branch, detached HEAD (12-character commit), or unborn branch; upstream
  with ↑/↓ counts marked "local ref, not fetched", "(no upstream)", or
  "upstream … missing locally".
- **Tab bar:** `1 STAGED n  2 UNSTAGED n  3 COMBINED n  4 UNTRACKED n`, the
  active one reversed, plus the comparison description.
- **Sidebar:** bordered `CHANGES (n)` list. Each row has a status code,
  a conflict `!`, the path (truncated from the start with `…`), and `+a -r`
  or `bin`. The selected row is reversed.
- **Diff panel:** bordered and titled with the file. A metadata line shows
  kind, conflict badge, rename source and similarity, mode or symlink or
  submodule, `+a -r`, hunk count, and a "bounded alignment" note. Below
  are column titles (`OLD · HEAD`, `NEW · index`, and so on), then rows.
- **Status line:** the message (colored by kind) plus position
  (`file i/n · hunk k/m · rows a-b of N · col c`).
- **Footer:** key hints.
- **Overlays:** help or error, centered, cleared underneath, and clipped to
  the screen.

Rows come from the cached `DiffView`. Each frame slices the visible window
and builds Ratatui `Line`s whose spans borrow the precomputed text (no
content copying). The two gutters and two content columns are separate
paragraphs. The content paragraphs share one horizontal offset; gutters
never scroll.

Styles are foreground-only standard colors plus `BOLD`/`REVERSED`:
green added, red removed, yellow `~` markers, cyan accents, dark gray
numbers and metadata, magenta escapes and conflicts. Meaning is also
carried by markers and letters.

## 7. Keyboard shortcuts

| Key | Action |
|---|---|
| `j`/`↓`, `k`/`↑` | next / previous file |
| `Home` / `End` | first / last file |
| `Ctrl+D` / `Ctrl+U` | file list page down / up |
| `Enter` | load or reload the selected file |
| `1` `2` `3` `4` | staged / unstaged / combined / untracked |
| `Tab` / `Shift+Tab` | next / previous comparison |
| `J` / `K` | scroll diff one line |
| `PgDn` / `Space` / `PgUp` | scroll diff one page |
| `g` / `G` | top / bottom |
| `]` / `[` | next / previous hunk |
| `l`/`→`, `h`/`←`, `0` | scroll right, left, back to column 0 |
| `b`, `<`, `>` | toggle sidebar, narrower, wider |
| `r` | refresh |
| `?` | toggle help |
| `Esc` | close overlay / clear message |
| `q`, `Ctrl+C` | quit |

Only key presses count (releases are ignored). Ctrl and Alt combinations
never alias plain keys. While the help overlay is open, only `?`, `Esc`,
`q`, and `Ctrl+C` act. The error overlay also accepts `Enter` (close) and
`r` (retry). The help overlay is generated from the same table
(`ui::overlays::KEYS`).

## 8. Comparison mode behavior

- Startup selects the first non-empty comparison, in the order unstaged,
  staged, untracked, combined.
- Switching resets the diff scroll, keeps that comparison's own selection,
  and loads its selected file lazily (from the cache if present).
- Each comparison is a separate Phase 2 `DiffFileList`; nothing is merged.
  `tui_app::comparison_modes_keep_partial_staging_distinct` asserts that
  the same file shows A→B (staged), B→C (unstaged), and A→C (combined).
- Untracked files are loaded by the Phase 2 untracked reader; nothing is
  staged.

## 9. Side-by-side rendering behavior

- One `ViewRow::Line` per Phase 2 `AlignedRow`; there is no re-alignment.
  `Context` shows both sides; `Removed` the left only; `Added` the right
  only; `Modified` both, with `~`.
- A missing side is an empty cell with no number. Numbers are Phase 2's.
- Hunk header rows span both panes. The selected hunk is `▶` and bold.
- Line endings: a missing final newline appends `\ no newline` (muted). A
  CRLF ending appends `␍`.
- Content: tabs expand to 4-column stops. C0 controls and DEL become
  control pictures (`␛`, `␇`). C1 and bidi/format controls become
  `<U+XXXX>`. Invalid bytes become `\xNN`. All escapes are magenta.
- Long lines: horizontal scrolling of both content columns together, in
  steps of 8 columns, bounded by the widest line. Gutters stay fixed.
  Nothing is wrapped or cut.
- Narrow panels: if two content columns of at least 10 cells cannot fit,
  the panel says so, with the needed width, and suggests `b` or `<`.

## 10. Lazy loading integration

| Moment | Git calls | Evidence |
|---|---:|---|
| `App::open` | 6 (rev-parse, status, 4 discoveries) | `startup_discovers_everything_but_loads_only_the_selected_file` |
| First `load_pending` | 1 | same test |
| Rendering, scrolling, hunks, sidebar, overlays, resize | 0 | `rendering_scrolling_and_resizing_never_run_git` |
| Returning to a loaded file | 0 | `returning_to_a_loaded_file_uses_the_cache` |
| `Enter` (explicit reload) | 1 | same test |
| `r` refresh | snapshot + 4 discoveries + 1 load | `refresh_*` tests |

No diff is loaded at startup other than the selected file's. Stale and
vanished outcomes from Phase 2 are cached as failures until refresh or
Enter, so outdated content is never presented.

## 11. Error handling

- **Before the TUI starts** (not a repository, Git missing, path invalid,
  no TTY): the error is printed normally to stderr with exit code 1. The
  terminal is never put into raw mode.
- **Per-comparison discovery failures** show in the sidebar ("Could not
  list changes: …"), with `!` in the tab bar.
- **Per-file load failures:**
  - Stale ("changed since the list was loaded … Press r to refresh") and
    Vanished are warnings.
  - Git, parse, I/O, and permission errors are errors.
  - Each shows in the diff panel and the status line. Messages are single
    lines, truncated at 300 characters, with control characters removed or
    escaped.
- **Refresh failures** open an error overlay (`Esc` close, `r` retry) and
  keep the previous state.
- **Small terminals** get a warning screen, never a panic.
  `rendering_never_panics_at_any_size` covers 99 sizes with overlays and
  scrolling.

## 12. Terminal lifecycle safety

`TerminalGuard::enter()` installs the panic hook once, then:
- enables raw mode;
- enters the alternate screen, hides the cursor, and clears.

Each step is recorded, so `Drop` undoes exactly what succeeded: it leaves
the alternate screen, shows the cursor, and disables raw mode. `run` drops
the guard after the loop, whether it ended by quit or by error. Unwinding
also drops it. The panic hook restores first and then calls the previous
hook. Mouse capture is never enabled. Nothing is logged while the
interface is active.

Ratatui's `Terminal::clear()` was found to send a cursor-position query
(`ESC[6n`). With no answer it failed after a timeout; the PTY test caught
this. The guard now clears with Crossterm's `Clear(All)`, and the tests
assert that `ESC[6n` is never sent.

Evidence:
- `tui_cli::interactive_session_in_a_real_pty`: the transcript starts with
  `ESC[?1049h`, ends with `ESC[?1049l ESC[?25h`, contains no `ESC[6n`, and
  the exit code is 0.
- `tui_cli::not_a_repository_fails_before_touching_the_terminal`.
- `tui_panic::panic_hook_chains_to_the_previous_hook`.

## 13. Responsive layout behavior

- **Minimum 40×10.** Below that, a warning shows the current and required
  size.
- **Sidebar:** default 36 columns, minimum 16, at most 40% of the width.
  It can be hidden with `b` and resized with `<`/`>`.
- **Diff panel:** columns are computed from the line-number width. A "too
  narrow" panel is reported, not squeezed.
- **Resize events:** update the size, re-clamp scroll and sidebar
  offsets, and redraw. They don't reload (asserted: 0 Git calls).
- **Tested sizes:** 120×40, 80×24, 50×20 (narrow fallback, then a full
  view after `b`), 39×30 and 120×9 (warning), 140×45, and 99 more in the
  no-panic test.

## 14. Automated tests

| Target | Tests | Covers |
|---|---:|---|
| lib unit `display` | 7 | escapes, tabs, truncation, path format controls |
| lib unit `tui::layout` | 5 | sizes, tiling, sidebar bounds, columns |
| lib unit `tui::keymap` | 3 | every binding, modifiers/releases, overlays |
| `tests/tui_app.rs` | 17 | state over real repos (see `TESTING_STRATEGY.md`) |
| `tests/tui_render.rs` | 14 | TestBackend rendering, colors, notices, sizes, escaping |
| `tests/tui_cli.rs` | 4 | routing; 3 real-PTY sessions via `script(1)` |
| `tests/tui_panic.rs` | 1 | panic hook chaining |
| **New total** | **51** | |

Full suite: 287 passed, 0 failed, 0 ignored.
- Library unit tests: 97 (82 before Phase 3, plus 15).
- Binary unit tests: 5.
- Integration tests: 185 (97 Phase 1, 52 Phase 2, 36 Phase 3).

## 15. Manual testing results

Human-operated testing in a terminal emulator (keyboard by hand, visual
judgment, real window resizing) **was not performed by me**. What was run
is listed honestly below.

| Checklist item | How verified | Result |
|---|---|---|
| Launch `diffkit` in a repository | Real PTY session (`script`, 110×30), keys sent programmatically | PASS (automated PTY) |
| Launch with explicit path | Real PTY session from another directory | PASS (automated PTY) |
| Switch all four modes | PTY keys `1`, `4`, `Tab`; state and render tests for all modes | PASS (automated) |
| Navigate with j/k, open with Enter | PTY `j`; state tests | PASS (automated) |
| Side-by-side changes | PTY transcript contains both sides; TestBackend assertions | PASS (automated) |
| Scroll long diffs, navigate hunks | PTY `]`, `G`; state tests | PASS (automated) |
| Open and close help | PTY `?` `?`; state test | PASS (automated) |
| Refresh after external changes | PTY `r` ("Refreshed" shown); state tests with external `git add` | PASS (automated) |
| Resize the terminal | `Resize` actions and render tests only; no real window resize | NOT TESTED in a real terminal |
| Quit cleanly | PTY: exit 0, alternate screen left, cursor shown | PASS (automated PTY) |
| Diagnostic subcommands | Existing and new CLI tests | PASS |
| Repository with no changes, binary files, conflicts | State and render tests over real repositories | PASS (automated) |
| Light and dark terminal themes | Not checked visually | NOT TESTED |

Recommended before sign-off: open `diffkit` by hand in your usual terminal
and theme and walk through the checklist above.

## 16. Performance observations

`cargo run --release --example bench_tui -- 7` (details in
`docs/PERFORMANCE_BUDGET.md`):

| Measure | Result |
|---|---:|
| Open + first diff, 100 / 5,000 / 50,000 tracked files | 30.6 / 44.2 / 312.6 ms |
| Git calls to first diff | 6 + 1 |
| Phase 2 load of a 200k-line file vs TUI first load | 573.5 ms vs 565.9 ms (display rows add 7.9 ms) |
| One frame (200×60), 16,000-row diff | 0.207 ms |
| Scroll or hunk jump + frame | 0.222 ms, 0 Git calls |
| Return to a loaded file | 0.000 ms, 0 Git calls |

Compared with Phase 2:
- No engine path changed.
- The interface adds display-row construction (about 0.5 µs per row).
- Startup discovers three extra comparisons, metadata only (largest cost
  at 50,000 files: about 300 ms total), to show tab counts.
- Frame time does not depend on diff size.

## 17. Known limitations

1. Diff loading is synchronous. A huge file blocks input while Git computes
   the diff; "Loading…" is drawn first. There is no cancellation.
2. No mouse support (deliberate: capture disables text selection). No
   syntax highlighting.
3. Not verified by hand in terminal emulators: themes, contrast, and real
   resizing.
4. Windows is compiled and linted but not executed; the PTY tests are
   macOS/Linux only, and the Linux `script` syntax is implemented but was
   not run here.
5. The cache is a fixed 16 entries, oldest-first; refresh clears it.
6. Tab stops (4 columns) are computed per character and are approximate
   after wide characters. Very long single lines (minified files) cost
   time proportional to their length when visible.
7. CRLF lines all show `␍`, which is informative but can be visually busy
   in CRLF-only files.
8. Startup always discovers all four comparisons.
9. A directory named like a subcommand (`inspect`) must be passed as
   `./inspect`.

## 18. Platform verification

| Platform | Build | Tests | Interactive |
|---|---|---|---|
| macOS 26.5 arm64 | PASS | 287/287 | Real PTY sessions (automated) PASS; human use not performed |
| Linux | not built here | not run | not run |
| Windows (`x86_64-pc-windows-msvc`) | `cargo clippy --all-targets -D warnings` PASS | not run | not run |

## 19. Exact validation command results

Run after the final code change, in order:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check` | exit 0, 0 warnings |
| `cargo build --release` | exit 0, 0 warnings |
| `cargo test` | exit 0; 287 passed, 0 failed, 0 ignored |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0, 0 warnings |
| Extra: `cargo clippy --target x86_64-pc-windows-msvc --all-targets --all-features -- -D warnings` | exit 0 |

Clippy findings during development were fixed, not suppressed: a type
alias in tests, and `#[cfg]` on platform-specific test imports. No `allow`
attributes were added.

## 20. Phase 1 and Phase 2 behavior

- `src/git/` and `src/diff/` were not modified; `git status` shows no
  changes under them.
- All 97 Phase 1 and 52 Phase 2 integration tests pass, as do their unit
  tests.
- Read-only audits still pass. `inspect` still runs only rev-parse,
  status, and for-each-ref; the diff engine only its 7 allowed
  subcommands.
- Intentional changes:
  1. `diffkit` with no arguments opens the viewer, as Phase 3 requires.
     Previously it printed usage. Without a terminal it now exits 1 with a
     message. Phase 1's `help_version_and_default` was updated for that
     case only; its `--help`, `-h`, and `--version` assertions are
     unchanged.
  2. `display_path` moved into the library unchanged, plus escaping of
     Unicode bidi and format controls (`U+200B–200F`, `U+202A–202E`,
     `U+2060–2064`, `U+2066–2069`, `U+FEFF`). `inspect` and `diff-stats`
     output differs only for names containing those characters. The three
     existing `display_path` tests pass unchanged.
  3. A single non-option argument that is an existing directory now opens
     the viewer. Previously every unknown argument was a usage error;
     non-directories still are (exit 2).

## 21. Architectural deviations

1. **Module location.** The TUI is in the library (`src/tui/`), not
   `src/app/state.rs`, `src/ui/`, and `src/terminal/`. This keeps the
   binary's existing `app` module and makes the interface testable from
   `tests/`.
2. **Startup discovery.** Startup discovers all four comparisons
   (metadata only) so tab counts are accurate; loading is still one file.
3. **Loading on selection.** `j`/`k` load the newly selected file lazily
   (after queued input). `Enter` forces a reload, bypassing the cache.
4. **`J`/`K` granularity.** `J`/`K` scroll one line; `PgDn`/`PgUp`/`Space`
   scroll a page. The assignment grouped `J` with `PageDown`.
5. **Additional keys:** `h`/`l`/`←`/`→`/`0` (horizontal), `b`/`<`/`>`
   (sidebar), `Space`, `Shift+Tab`, `Ctrl+C`.
6. **`unicode-width`** added (already a Ratatui dependency).
7. **Clearing.** The alternate screen is cleared with Crossterm instead of
   `Terminal::clear()`, to avoid the cursor-position query.

## 22. Files changed

Modified:
- `Cargo.toml`, `Cargo.lock`
- `src/lib.rs`, `src/app/cli.rs`, `src/app/report.rs`
- `tests/common/mod.rs`, `tests/cli_inspect.rs`
- `README.md`
- `docs/ARCHITECTURE.md`, `docs/ENGINEERING_RULES.md`,
  `docs/PERFORMANCE_BUDGET.md`, `docs/TECH_STACK.md`,
  `docs/TESTING_STRATEGY.md`, `docs/phases/PHASE_3_TERMINAL_UI.md`

Added:
- `src/display.rs`
- `src/tui/{mod,app,actions,keymap,diff_view,layout,terminal}.rs`
- `src/tui/ui/{mod,header,sidebar,diff_panel,status,overlays,theme}.rs`
- `tests/tui_app.rs`, `tests/tui_render.rs`, `tests/tui_cli.rs`,
  `tests/tui_panic.rs`
- `examples/bench_tui.rs`
- `docs/PHASE_3_IMPLEMENTATION_REPORT.md`

## 23. Definition of Done

| Requirement | Status | Evidence |
|---|---|---|
| `diffkit` launches an interactive terminal application | PASS | `tui_cli::interactive_session_in_a_real_pty` |
| Discovers the current repository | PASS | PTY session; `App::open` tests |
| Header shows repository and branch | PASS | `normal_layout_shows_every_region`, `header_shows_tracking_and_detached_states` |
| Sidebar shows changed files | PASS | `normal_layout_…`, `large_sidebar_keeps_selection_visible` |
| All four comparison modes work | PASS | `comparison_modes_keep_partial_staging_distinct`, `full_flow_over_every_kind_of_change` |
| File selection works | PASS | `file_selection_moves_within_bounds`, PTY `j` |
| Side-by-side rendering works | PASS | `side_by_side_rows_numbers_markers_and_colors` |
| Old and new line numbers are correct | PASS | same; `added_and_deleted_files_use_one_side`; `tui_app` row assertions |
| Additions and deletions are distinguishable | PASS | Colors asserted (red/green) plus `-`/`+`/`~` markers |
| Long diffs can be navigated | PASS | `vertical_and_horizontal_scroll_are_bounded`, `long_lines_scroll_horizontally_with_fixed_line_numbers` |
| Hunk navigation works | PASS | `hunk_navigation`, `hunk_navigation_without_hunks_is_a_no_op` |
| Binary and special files represented | PASS | `binary_conflict_mode_rename_and_empty_notices`, `full_flow_…` |
| Errors handled without crashing | PASS | `stale_file_is_reported_not_shown`, `not_a_repository_fails_before_touching_the_terminal`, `empty_repository_changes_are_handled` |
| Terminal dimensions handled safely | PASS | `rendering_never_panics_at_any_size`, `narrow_terminal_falls_back_clearly`, `too_small_terminals_show_a_warning`, `resizing_keeps_scroll_valid` |
| Terminal state restored on exit | PASS | PTY transcript checks; `tui_panic`; guard design (§12) |
| Diff loading remains lazy | PASS | `startup_discovers_everything_but_loads_only_the_selected_file` |
| Rendering does not invoke Git | PASS | `rendering_scrolling_and_resizing_never_run_git` |
| Existing CLI diagnostic commands work | PASS | Phase 1 and 2 CLI tests; `routing_without_a_terminal` |
| Phase 1 and Phase 2 tests pass | PASS | 149 integration tests plus unit tests |
| New automated tests pass | PASS | 51 new, all pass |
| Documentation updated | PASS | §22 |
| All five Cargo commands pass | PASS | §19 |
| Human-operated manual testing | NOT TESTED | §15 (automated PTY equivalents run) |
| Windows execution | NOT TESTED | §18 |

Phase 3 is ready for review. Phase 4 has not been started.
