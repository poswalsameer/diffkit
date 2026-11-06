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
- `--ahead-behind` (Git 2.17+) and porcelain v2 (Git 2.11+) are covered by
  the 2.31 minimum.

## Approved dependencies

None. No dependency was added in Phase 1.
