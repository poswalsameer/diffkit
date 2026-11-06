# Diffkit

A read-only, offline Git change viewer for the terminal, written in Rust on
top of the installed `git` executable. It shows staged, unstaged, combined,
and untracked changes side by side, and never modifies your repository.

## Build

Requirements:

- Rust stable (1.88 or newer for the Ratatui dependency; developed with 1.98).
- Git 2.31 or newer at runtime (2.45+ for the partial-clone no-fetch guard).

```sh
cargo build --release
# the binary is target/release/diffkit; copy it anywhere on your PATH
```

## Launch

```sh
diffkit                    # interactive viewer for the repository you are in
diffkit path/to/repo       # interactive viewer for another repository
diffkit inspect            # plain-text repository report (non-interactive)
diffkit diff-stats [--load]  # plain-text list of changed files per comparison
diffkit --help | --version
```

The interactive viewer needs a terminal. When stdout or stdin is not a TTY,
`diffkit` exits with a message pointing to `inspect` and `diff-stats`.
Subcommand names take priority over directory names; use `./inspect` to
open a directory named `inspect`.

## What it looks like

This is real output of the renderer, captured with Ratatui's test backend
at 110×24 from a test repository (not a screenshot of a terminal emulator).
Colors are not shown: added text is green, removed red, and `~` pairs are
yellow markers.

```text
 DIFFKIT  my-project │ ⎇ main (no upstream)
 1 STAGED 1   2 UNSTAGED 2   3 COMBINED 3   4 UNTRACKED 1  index -> working tree
┌ CHANGES (2) ─────────────────────┐┌ src/app.js ────────────────────────────────────────────────────────────┐
│M  README.md                 +1 -2││Modified · 100644 · +3 -2 · 1 hunk                                      │
│M  src/app.js                +3 -2││      OLD · index                         NEW · working tree            │
│                                  ││▶ @@ -1,7 +1,8 @@                                                       │
│                                  ││  1   const name = "Alice";        │  1   const name = "Alice";         │
│                                  ││  2 ~ const age = 20;              │  2 ~ const age = 21;               │
│                                  ││  3   console.log(name);           │  3   console.log(name);            │
│                                  ││                                   │  4 + console.log(age);             │
│                                  ││  4                                │  5                                 │
│                                  ││  5   function greet() {           │  6   function greet() {            │
│                                  ││  6 ~   return `hi ${name}`;       │  7 ~   return `hello ${name}`;     │
│                                  ││  7   }                            │  8   }                             │
└──────────────────────────────────┘└────────────────────────────────────────────────────────────────────────┘
file 2/2 · hunk 1/1 · rows 1-9 of 9
j/k files  J/K PgUp/PgDn scroll  [ ] hunks  h/l left/right  1-4 Tab comparison  r refresh  ? help  q quit
```

(Empty rows in the middle of the original 24-line capture are omitted.)

## Features

- **Four comparisons**, never merged:
  - STAGED: HEAD → index.
  - UNSTAGED: index → working tree.
  - COMBINED: HEAD → working tree.
  - UNTRACKED: new files, read directly and never staged.
- **Side-by-side diff:**
  - Old content left and new content right, each with its own line numbers.
  - Markers on each line: `-` removed, `+` added, `~` paired modification.
  - Hunk headers, with the selected hunk marked `▶`.
  - Markers for a missing final newline (`\ no newline`) and CRLF endings (`␍`).
- **File list** with status letters:
  - `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied,
    `T` type changed, `X` mode changed, `U` unmerged, `??` untracked.
  - A trailing `!` marks a conflicted file.
  - `+`/`-` line counts, or `bin` for binary files.
- **Header:** repository name, branch or detached HEAD, worktree and shallow
  markers. Ahead/behind counts against the locally known upstream are
  labeled "local ref, not fetched".
- **Special files** each get a clear explanation instead of text:
  - binary files (with sizes), submodules, conflicts, mode-only changes;
  - pure renames, empty files, oversized diffs, unreadable files, nested
    repositories.
- **Safe display:** filenames and content are escaped, so control
  characters, terminal escape sequences, bidi overrides, and invalid UTF-8
  are shown visibly and never reach the terminal.
- **Lazy and cheap:**
  - Startup lists files without loading any diff, then loads only the
    selected file.
  - Recently viewed diffs (up to 16) are kept.
  - Rendering, scrolling, and resizing never run Git.
- **Responsive layout:**
  - Adapts to any size: the sidebar is limited to 40% of the width.
  - Narrow panels get a clear "too narrow" message.
  - Terminals below 40×10 get a resize warning.

## Keys

| Key | Action |
|---|---|
| `j` / `↓`, `k` / `↑` | next / previous file |
| `Home` / `End` | first / last file |
| `Ctrl+D` / `Ctrl+U` | page down / up in the file list |
| `Enter` | load (or reload) the selected file |
| `1` `2` `3` `4` | staged / unstaged / combined / untracked |
| `Tab` / `Shift+Tab` | next / previous comparison |
| `J` / `K` | scroll the diff one line |
| `PgDn` / `Space` / `PgUp` | scroll the diff one page |
| `g` / `G` | top / bottom of the diff |
| `]` / `[` | next / previous hunk |
| `l` / `→`, `h` / `←` | scroll right / left |
| `0` | back to the first column |
| `b` | show / hide the file list |
| `<` / `>` | narrower / wider file list |
| `r` | refresh from Git |
| `?` | help |
| `Esc` | close overlay / clear message |
| `q` / `Ctrl+C` | quit |

## Known limitations

- No automatic refresh: press `r` after changing the repository elsewhere
  (watching is Phase 4).
- No mouse support (mouse capture would disable the terminal's own text
  selection). No syntax highlighting.
- The viewer has been tested in pseudo-terminals and Ratatui's test
  backend on macOS. Windows builds and passes lint but has not been run.
- Colors use the terminal's 16-color palette (foreground only), so they
  follow your theme. Contrast has not been checked in specific terminal
  themes.
- Diff loading is synchronous: a very large file (hundreds of thousands of
  lines) shows "Loading…" while Git computes the diff.

## Documentation and development

See `docs/`: architecture, data model, engineering rules, testing strategy,
performance measurements, and the Phase 1, 2, and 3 implementation reports.

```sh
cargo test                                            # unit, integration, and PTY tests
cargo run --release --example bench_inspect -- 15     # Phase 1 benchmark
cargo run --release --example bench_diff -- 7         # Phase 2 benchmark
cargo run --release --example bench_tui -- 7          # Phase 3 benchmark
```
