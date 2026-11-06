# Diffkit

A read-only, offline Git repository inspector for the terminal, written in
Rust on top of the installed `git` executable.

```sh
cargo build --release
./target/release/diffkit inspect     # run from anywhere inside a working tree
./target/release/diffkit diff-stats   # changed files per comparison; --load parses every diff
./target/release/diffkit --help
./target/release/diffkit --version
```

`diffkit inspect` reports the repository root, Git directory, and worktree
type. It shows the current branch or detached HEAD, the upstream and
ahead/behind counts against the *locally known* upstream (never fetched, so
possibly stale), and staged, unstaged, partially staged, untracked,
conflicted, deleted, renamed, and copied files.

Requirements: Git 2.31 or newer (2.45+ for the partial-clone no-fetch
guard). There are no other runtime dependencies.

Phase 1 (the Git intelligence engine) and Phase 2 (the diff engine, a
library API in `src/diff/`) are implemented. There is no diff rendering and
no interactive UI yet; that is Phase 3. See `docs/` for the architecture, data model, engineering rules, testing
strategy, performance measurements, and the Phase 1 and Phase 2
implementation reports.

```sh
cargo test                                            # unit and integration tests
cargo run --release --example bench_inspect -- 15     # Phase 1 benchmark
cargo run --release --example bench_diff -- 7         # Phase 2 benchmark
```
