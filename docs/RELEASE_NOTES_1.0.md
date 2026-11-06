# Diffkit 1.0.0 — release notes (draft)

Status: **draft, not released.** The version is set to 1.0.0 in
`Cargo.toml`, but nothing has been tagged or published.
[RELEASE_CHECKLIST.md](RELEASE_CHECKLIST.md) lists what remains before a
release.

## What Diffkit is

A read-only, offline terminal viewer for Git changes.

- It shows staged, unstaged, combined (HEAD → working tree), and untracked
  changes side by side.
- It keeps itself current as files and Git state change.
- It never writes to your repository and never contacts a remote.

## Highlights

- Four comparisons, never merged, with Myers-aligned side-by-side diffs.
- Automatic synchronization:
  - 200 ms quiet period, at most 1 s;
  - one Git re-read per burst of changes;
  - the selected file and scroll position are kept.
- A bounded, revalidated diff cache (64 MiB).
- Clear explanations for:
  - binary, empty, large, and unreadable files;
  - conflicts, with the base/ours/theirs stages;
  - submodules and nested repositories;
  - symlinks, renames, and copies;
  - mode and type changes.
- Operations in progress (merge, rebase, am, cherry-pick, revert, bisect),
  detached and unborn HEAD, linked worktrees, shallow clones.
- Local history: the 200 latest commits, each inspectable file by file.
- Plain-text commands for scripts: `inspect`, `diff-stats`, `show`.
- Every displayed string is escaped. The terminal is restored on every
  exit path Diffkit can observe.

## Platforms

Verified on macOS (Apple Silicon) only. Builds for macOS Intel, Linux, and
Windows compile and pass lint but have not been run. See the README's
"Supported platforms" table.

## Install

Build from source with Rust 1.88+: `cargo install --path .`. Git 2.31+ must
be on `PATH`.

## Known limitations

See the README: Linux inotify watch limits, network filesystems, Git
helpers that Git itself may run (LFS filters, fsmonitor), history limited
to 200 commits on HEAD, and platforms not yet run.
