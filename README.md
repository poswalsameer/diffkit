# Diffkit

A read-only, offline Git change viewer for the terminal, written in Rust on
top of the installed `git` executable. It shows staged, unstaged, combined,
and untracked changes side by side, follows your repository as it changes,
explains conflicts, submodules, and operations in progress, lets you
inspect recent local commits, and never modifies your repository.

Version 1.0.0. See [CHANGELOG.md](CHANGELOG.md) and
[docs/RELEASE_NOTES_1.0.md](docs/RELEASE_NOTES_1.0.md).

## Install

Requirements:

- **Git** on `PATH` at runtime: 2.31 or newer (2.45+ for the partial-clone
  no-fetch guard). Developed and tested with Git 2.54.
- **To build:** Rust stable 1.88 or newer (the minimum declared by the
  Ratatui dependency; developed with 1.98). No other system libraries.

Diffkit is a single executable with no runtime files, services, or
configuration. No prebuilt binaries are published yet; build from source.

### macOS and Linux

```sh
git clone <this repository> diffkit && cd diffkit
cargo install --path .          # installs ~/.cargo/bin/diffkit
# or: cargo build --release && cp target/release/diffkit ~/.local/bin/
diffkit --version
```

### Windows (PowerShell)

```powershell
git clone <this repository> diffkit; cd diffkit
cargo install --path .          # installs %USERPROFILE%\.cargo\bin\diffkit.exe
diffkit --version
```

Building on Windows needs the MSVC build tools that `rustup` asks for.
Windows support is **compiled and linted only**; it has not been run
(see [Supported platforms](#supported-platforms)).

To uninstall: `cargo uninstall diffkit`, or delete the binary you copied.

## Launch

```sh
diffkit                    # interactive viewer for the repository you are in
diffkit path/to/repo       # interactive viewer for another repository
diffkit --no-watch [PATH]  # same, without automatic synchronization (r refreshes)
diffkit inspect            # plain-text repository report (non-interactive)
diffkit diff-stats [--load]  # plain-text list of changed files per comparison
diffkit show <COMMIT>      # plain-text metadata and changed files of one local commit
diffkit --help | --version
```

`show` accepts anything Git resolves to a local commit: a full or
abbreviated hash, a branch, a tag, or an expression such as `HEAD~2`.

Exit codes: `0` success; `1` inspection failed (for example, not a Git
repository, or no such commit); `2` usage error (unknown command or
option); `128 + N` when the interactive viewer was stopped by signal `N`
(for example `143` for SIGTERM) after restoring the terminal.

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
j/k files  J/K PgUp/PgDn scroll  [ ] hunks  h/l left/right  1-4 Tab comparison  L history  r refresh  ? help
```

(Empty rows in the middle of the original 24-line capture are omitted. The
footer is cut at the terminal width; `q quit` follows on wider terminals.)

## Automatic synchronization

While the viewer is open, it watches the working tree and the repository's
Git metadata and refreshes itself when anything changes outside Diffkit:
- saving in an editor;
- creating, deleting, or renaming files;
- staging, unstaging, committing, or switching branches in another terminal
  or Git client;
- merges, rebases, and conflicts;
- a linked worktree's shared references.

How it works:
- Filesystem events are only a hint. After a burst of changes goes quiet
  for 200 ms, Diffkit re-reads the state from Git. Continuous activity
  still syncs at least once a second.
- The new state replaces the old one in a single step. Your comparison,
  selected file, and scroll position are kept when they still exist.
- Cached diffs are dropped only for files whose Git metadata changed.

The status bar shows:

| Indicator | Meaning |
|---|---|
| `● Synced` | The view matches Git. |
| `◌ Changes detected` | Changes were seen and will be applied shortly. |
| `↻ Synchronizing` | Re-reading from Git (off the input thread; scrolling keeps working). |
| `! Watcher unavailable` | File watching failed. Press `r` to refresh manually. |
| `! Refresh required` | The last sync failed. The view may be outdated; press `r`. |
| `○ Auto-sync off` | Started with `--no-watch`. |

`r` uses the same pipeline immediately. `D` shows synchronization and
cache diagnostics.

Not watched:
- untracked directories that your ignore rules exclude entirely (for
  example `target/` or `node_modules/` when ignored);
- Git's object database, logs, and hooks.

A tracked directory is never excluded, whatever its name. Diffkit never
fetches, so ahead/behind counts still reflect your local remote-tracking
refs.

## Repository states

The header shows what Git is in the middle of, read from the same state
files `git status` uses, in this worktree only:

| Header | Meaning |
|---|---|
| `MERGING` | A merge stopped (for example, on conflicts). |
| `REBASING 2/5` | A rebase is at step 2 of 5. `git rebase` and `git rebase -i` leave identical state, so they are not told apart. |
| `APPLYING PATCHES (am) 1/3` | `git am` is applying a mailbox. |
| `CHERRY-PICKING`, `REVERTING` | A cherry-pick or revert stopped. |
| `BISECTING` | `git bisect` has started. |
| `⚠ 3 conflicts` | Unmerged paths in the index. |
| `HEAD detached at <commit>` | No branch is checked out. |
| `⎇ main (no commits yet)` | The branch has no commits (unborn). |

`diffkit inspect` reports the same, plus every conflict's index stages.

Special entries are explained in the diff panel rather than shown as
errors:

- **Conflicts:** Git's conflict kind (both modified, added by us, deleted
  by them, …), which index stages exist (base, ours, theirs; missing
  stages are shown as missing), and whether a working-tree file exists.
  Diffkit never resolves anything.
- **Submodules and embedded repositories (gitlinks):** the recorded commit
  before and after, whether the submodule is checked out, not initialized,
  or missing, and what Git reports about its contents. Their files are
  never listed, initialized, or updated.
- **Untracked nested repositories:** listed once, never descended into.
- **Symlinks:** the target is shown as text and never followed, including
  broken links and links leaving the repository. A file replaced by a
  symlink (or the reverse) is a type change: `symlink → regular file`.
- **Renames and copies:** only as Git reports them, with both paths and
  the similarity. An unrelated deletion and addition stay separate.

## Local history

Press `L` for the most recent 200 commits reachable from HEAD (local
history only; nothing is fetched). The right panel shows the selected
commit's full hash, parents, author, committer, dates, and message.

- `Enter` lists the files that commit changed, compared with its first
  parent (merges say so; a root commit is compared with the empty tree).
  Each file opens in the same side-by-side view, with binary, mode,
  rename, and submodule changes explained as above.
- `Esc` goes back: from a commit to the list, from the list to the working
  tree, which is shown exactly as you left it (comparison, file, scroll).
  Automatic synchronization keeps running underneath.
- In a shallow clone, commits on the boundary say their parents are not
  available locally instead of showing a misleading diff.

`diffkit show <COMMIT>` prints the same information as plain text.

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
  markers, operations in progress, and the conflict count. Ahead/behind
  counts compare with the upstream **as last fetched** (a remote-tracking
  ref, or a local branch); Diffkit never fetches, so they can be stale.
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
  - Recently viewed diffs are kept in a 64 MiB least-recently-used cache.
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
| `r` | refresh from Git now (on the history screen: reload the list) |
| `D` | synchronization and cache diagnostics |
| `L` | open / close local history |
| `Enter` (history) | list the selected commit's files |
| `Esc` (history) | commit → history list → working tree |
| `?` | help |
| `Esc` | close overlay / clear message |
| `q` / `Ctrl+C` | quit |

## Supported platforms

| Platform | Build | Tests | Interactive use | Status |
|---|---|---|---|---|
| macOS 26, Apple Silicon (aarch64) | yes | full suite, including real watcher and pseudo-terminal sessions | automated pseudo-terminal sessions | **Verified** |
| macOS, Intel (x86_64) | compiles and links | not run | not run | Unverified |
| Linux x86_64 / ARM64 | compiles and lints | not run | not run | Unverified |
| Windows x86_64 | compiles and lints | not run | not run | Unverified |

"Compiles" means `cargo clippy --target …` succeeded on macOS; no Linux or
Windows machine was available to run anything. Reports from those
platforms are welcome.

## Security and read-only behavior

- Diffkit runs only the `git` executable, never a shell, with arguments
  passed separately. Its Git commands are read-only:
  - `rev-parse`, `status`, `for-each-ref`;
  - `diff-index`, `diff-files`, `diff-tree`;
  - `ls-files`, `cat-file`, `check-ignore`, `log`.
- It never runs `add`, `commit`, `reset`, `checkout`, `switch`, `merge`,
  `rebase`, `stash`, `cherry-pick`, `fetch`, `pull`, or `push`, and never
  contacts a remote.
- Every Git call sets:
  - `GIT_OPTIONAL_LOCKS=0`, so `status` does not refresh the index;
  - `GIT_NO_LAZY_FETCH=1`, so a partial clone does not download objects;
  - literal pathspecs, so file names are never patterns.
- Diffs use `--no-ext-diff --no-textconv`; history uses
  `--no-show-signature`.
- The test suite verifies that `.git` and the working tree are unchanged
  byte for byte after inspection, and audits every Git process the running
  viewer starts.
- **What Git itself may still run:** repository-configured clean/smudge
  filters (for example Git LFS) and a `core.fsmonitor` hook can run while
  Git computes status or diffs, and Git checks initialized submodules'
  status. These are your repository's configuration; Diffkit cannot turn
  them off without changing results.
- Everything shown on screen is escaped:
  - control characters and terminal escape sequences;
  - bidirectional and zero-width characters;
  - invalid UTF-8;

  in file names, content, branch names, commit messages, author names, and
  error messages alike.
- Symlinks are never followed when reading; untracked reads stay inside
  the repository.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `the interactive interface needs a terminal` | stdout or stdin is redirected. Use `diffkit inspect`, `diff-stats`, or `show` for plain text. |
| `… is not inside a Git repository` | Run Diffkit inside a working tree, or pass its path. A `.git` file pointing at a missing directory gives the same error. |
| `! Watcher unavailable` | The OS refused file watching (often the Linux inotify limit: raise `fs.inotify.max_user_watches`). Press `r` to refresh. |
| `! Refresh required` | The last refresh failed (for example, the repository was moved). The view is the last good state; press `r`. |
| Status stays `◌ Changes detected` during a long Git operation | Git kept changing the index or refs while Diffkit read them; Diffkit waits for a consistent state instead of showing a mixed one. It refreshes once Git is idle. |
| Ahead/behind counts look wrong | They use the last fetched remote-tracking ref. Run `git fetch` yourself; Diffkit never fetches. |
| Slow status in a repository with Git LFS or an fsmonitor hook | Git runs those helpers itself; check `git status` alone for the same delay. |
| Terminal left in a strange state | Should not happen (normal exit, errors, panics, SIGTERM, SIGHUP, and SIGINT all restore it). If it does, run `reset` and please report it. |

## Known limitations

- Watching was verified on macOS (FSEvents) only. On Linux (inotify),
  every directory of the working tree gets a watch, including ignored ones,
  so very large trees can exceed `fs.inotify.max_user_watches`. Diffkit
  then shows "Watcher unavailable" and keeps working with `r`. Network
  filesystems often deliver no events; use `r` or `--no-watch`.
- Git itself may run repository-configured helpers while Diffkit reads
  state: clean/smudge filters such as Git LFS, and `core.fsmonitor` hooks.
  Diffkit disables external diff and textconv programs, but cannot
  disable these.
- Changes to a global `core.excludesFile` are noticed at the next refresh,
  not immediately.
- No mouse support (mouse capture would disable the terminal's own text
  selection). No syntax highlighting.
- The viewer has been tested in pseudo-terminals and Ratatui's test
  backend on macOS. Linux and Windows build and pass lint but have not been
  run. No human has completed the manual test checklist yet.
- History lists at most 200 commits and follows HEAD only (no branch
  picker, graph, search, or arbitrary ranges). A merge is shown against its
  first parent only.
- `SIGKILL` cannot be caught; after `kill -9` run `reset` in the terminal.
- Colors use the terminal's 16-color palette (foreground only), so they
  follow your theme. Contrast has not been checked in specific terminal
  themes.
- Diff loading is synchronous: a very large file (hundreds of thousands of
  lines) shows "Loading…" while Git computes the diff.

## Documentation and development

See `docs/`:
- architecture, data model, engineering rules, testing strategy;
- performance measurements;
- the Phase 1–5 implementation reports;
- the [release checklist](docs/RELEASE_CHECKLIST.md).

```sh
cargo test                                            # unit, integration, and PTY tests
cargo run --release --example bench_inspect -- 15     # Phase 1 benchmark
cargo run --release --example bench_diff -- 7         # Phase 2 benchmark
cargo run --release --example bench_tui -- 7          # Phase 3 benchmark
cargo run --release --example bench_sync -- 5         # Phase 4 benchmark
cargo run --release --example bench_release -- 5      # Phase 5 benchmark
```
