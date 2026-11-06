# Diffkit 1.0 release checklist

Updated 2026-11-07 at the end of Phase 5. Statuses:
- **Done** — verified, with evidence;
- **Open** — not yet done;
- **Blocked** — needs an environment or a decision this work did not have.

**Overall: not ready to release.** Platform runtime verification and the
human manual checklist are open, and a license has not been chosen.

## Build

| Item | Status | Evidence / next step |
|---|---|---|
| `cargo fmt --all -- --check` | Done | Exit 0 (Phase 5 report §23) |
| `cargo check --all-targets` | Done | Exit 0, no warnings |
| `cargo build --release` | Done | Exit 0; macOS arm64 binary 1,377,392 bytes |
| `cargo clippy --all-targets --all-features -- -D warnings` | Done | Exit 0 on the host |
| Clippy for Windows x86_64, Linux x86_64, Linux ARM64, macOS x86_64 | Done | Exit 0 for each `--target` |
| Release binary, macOS arm64 | Done | Built and run |
| Release binary, macOS x86_64 | Done (build only) | Links to a Mach-O x86_64 binary; not run (no Rosetta here) |
| Release binary, Linux x86_64 / ARM64 | Blocked | No Linux linker or sysroot on the build machine; build on Linux or in CI |
| Release binary, Windows x86_64 | Blocked | `link.exe` (MSVC) not available; build on Windows or in CI |
| Minimum Rust version 1.88 | Open | Declared from the dependencies' highest `rust-version`; not built with 1.88 |
| Minimum Git version 2.31 | Open | Only Git 2.54 was tested |

## Tests

| Item | Status | Evidence / next step |
|---|---|---|
| Full suite on macOS | Done | 412 passed, 0 failed, 0 ignored (`cargo test --all-targets`) |
| Earlier phases' tests preserved | Done | 347 → 412; none removed; changed assertions listed in TESTING_STRATEGY |
| Repeated timing-sensitive tests | Done | 6 consecutive runs, 0 failures |
| Read-only proof | Done | Fingerprint and Git-audit tests; see the report §F |
| Test suite on Linux | Open | Run `cargo test` on Linux (inotify backend) |
| Test suite on Windows | Open | Run `cargo test` on Windows; PTY and symlink tests are Unix-only |

## Runtime and installation

| Item | Status | Evidence / next step |
|---|---|---|
| macOS: interactive sessions | Done (automated) | Real-PTY sessions via `script(1)`: navigation, history, sync, signals |
| macOS: manual checklist by a person | Open | Phase 4 report §18 plus Phase 5: history, conflicts, operations, resize |
| Linux terminal emulator | Open | Keyboard, colors, resize, restore, watcher limits |
| Windows Terminal and PowerShell | Open | Keyboard, colors, resize, restore, ReadDirectoryChangesW |
| `cargo install --path .` | Done (macOS) | Installed into a scratch `--root` (not the user's `~/.cargo/bin`); the installed `diffkit --version` prints `diffkit 1.0.0` |
| Install on Linux and Windows | Open | Follow the README on each platform |

## Documentation, dependencies, security

| Item | Status | Evidence / next step |
|---|---|---|
| README, CHANGELOG, release notes | Done | Describe what is implemented and verified |
| Dependency review | Done | No new crates; `signal-hook` was already a Crossterm dependency |
| Vulnerability scan (`cargo audit`) | Open | Not run: requires installing cargo-audit and fetching the advisory database |
| License | Blocked | No license file; the owner must choose one before publishing |
| Security review of displayed text, Git calls, and symlinks | Done | Report §F; tests in `tui_states`, `tui_history`, `advanced_symlinks`, `history`, `tui_cli` |
| Known limitations documented | Done | README "Known limitations" and report §H |

## Release artifacts

| Item | Status | Evidence / next step |
|---|---|---|
| Artifacts for each supported target | Open | Only macOS arm64 (run) and x86_64 (not run) were produced |
| Checksums and signing | Open | Not set up |
| Tag `v1.0.0` and publication | Not done | Only on explicit instruction |
