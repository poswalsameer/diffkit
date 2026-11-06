# Technology Stack

| Component | Choice | Notes |
|---|---|---|
| Language | Rust, edition 2021 | Developed with stable rustc 1.98.1. Uses `<[u8]>::utf8_chunks` (Rust 1.79+) and `std::path::absolute` (1.79+). |
| Build | Cargo | `cargo build --release`; no build script. |
| Git integration | Installed `git` executable via `std::process::Command` | No libgit2, git2, gix, or other Git library. Tested with Git 2.54.0. |
| Libraries | Rust standard library only | `Cargo.toml` has no `[dependencies]`; `Cargo.lock` lists only `diffkit`. |
| Tests | Built-in `#[test]`, integration tests in `tests/` | No test dependencies. |
| Benchmarks | `examples/bench_inspect.rs` (std only) | Uses `/usr/bin/time` for memory figures where available. |

## Git version requirements

- **Git 2.31+** is required: discovery uses `rev-parse --path-format=absolute`.
  With an older Git the answer cannot be parsed and inspection fails with an
  "unexpected Git output (rev-parse)" error.
- **Git 2.45+** is needed for the partial-clone no-fetch guard
  (`GIT_NO_LAZY_FETCH`). Older versions ignore the variable, so a partial
  clone *could* lazily fetch a missing blob during `git status`.
- `--ahead-behind` (Git 2.17+), porcelain v2 (Git 2.11+),
  `--show-object-format` (Git 2.25+), and `--diff-algorithm=histogram` are
  covered by the 2.31 minimum.
- The diff engine (Phase 2) adds no dependency: the unified diff parser and
  Myers alignment are implemented in `src/diff/` with the standard library.

## Approved dependencies

Phase 1 and Phase 2 use only the standard library. Phase 3 adds three
dependencies, each required by the terminal interface:

| Crate | Version | Purpose |
|---|---|---|
| `ratatui` | 0.30.2, `default-features = false`, features `crossterm_0_29`, `layout-cache` | Terminal UI rendering (layout, widgets, buffers, test backend). The calendar widget and macros are excluded. |
| `crossterm` | 0.29.0 | Raw mode, alternate screen, cursor, and keyboard/resize events. The same version Ratatui's backend uses, so one copy is built. |
| `unicode-width` | 0.2.3 | Display width of characters, for truncating file names and expanding tabs correctly with wide (CJK, emoji) characters. Already a Ratatui dependency at this version; listing it adds no new code. |

Transitive dependencies come from these crates (`cargo tree`). There is no
async runtime, Git library, database, network client, or syntax
highlighter.
