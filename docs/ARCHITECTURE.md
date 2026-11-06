# Architecture

Diffkit is a library crate (`src/lib.rs`, module `git`) plus a binary
(`src/main.rs`, module `app`). The library owns everything about Git; the
binary owns argument handling and presentation.

```text
src/
  main.rs            entry point: passes argv to app::cli and exits with its code
  lib.rs             exposes the `git` module to the binary, tests, and later phases
  app/
    cli.rs           argument parsing, dispatch, stdout/stderr, exit codes
    report.rs        formats a RepositorySnapshot as the text report
  git/
    commands.rs      the only place a process is started
    errors.rs        GitError and Result
    models.rs        typed state shared by all layers
    repository.rs    repository discovery
    status.rs        porcelain v2 reading and byte-level parsing
    branches.rs      branch, HEAD, and upstream resolution
    commits.rs       ahead/behind tracking
    snapshot.rs      assembles one RepositorySnapshot
```

## Layers and responsibilities

| Layer | Module | Responsibility |
|---|---|---|
| CLI | `app::cli` | Reads arguments, chooses the command, prints the report or the error, returns exit codes (0 ok, 1 inspection failed, 2 usage error). Contains no Git logic. |
| Presentation | `app::report` | Turns a snapshot into text. Escapes ambiguous paths for display only. |
| Git command | `git::commands` | Runs `git -C <dir> <args…>` with `std::process::Command`, discrete `OsStr` arguments, closed stdin, `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_LAZY_FETCH=1`. Maps spawn failures to `GitNotFound` / `GitNotExecutable` / `Io`, non-zero exits to `CommandFailed` with stderr. Converts Git path bytes to native paths. |
| Repository | `git::repository` | One `rev-parse` answering inside-work-tree, bare, shallow, Git dir, common dir, and toplevel. Classifies failures as not a repository, not a work tree, or a command failure. |
| Status | `git::status` | One `git status --porcelain=v2 -z --branch --ahead-behind --untracked-files=all`. Parses NUL-terminated records as bytes into `FileStatus` values and keeps the `# branch.*` headers raw. |
| Branch | `git::branches` | Interprets `branch.oid`/`branch.head` into `BranchInfo`; resolves the upstream (`UpstreamInfo`) using `branch.upstream`, `branch.ab`, and at most one `rev-parse`. |
| Commit | `git::commits` | Interprets `branch.ab` into `CommitTrackingInfo`, or records why counts are unavailable. |
| Snapshot | `git::snapshot` | Calls the layers in order and returns one `RepositorySnapshot`. |
| Model | `git::models` | Plain data types and predicates; no I/O and no formatting. |

## Data flow for `diffkit inspect`

```text
cli::run
  └─ snapshot::inspect_current_dir
       ├─ repository::discover_from(cwd)   git rev-parse …                 (1 call; 3 more only if a path contains '\n')
       ├─ status::read(root)               git status --porcelain=v2 …     (1 call)
       ├─ branches::resolve                from headers; git for-each-ref  (only when HEAD is unborn)
       ├─ branches::resolve_upstream       git rev-parse @{upstream} …     (only when an upstream ref exists, or HEAD is unborn)
       └─ commits::tracking                from the branch.ab header       (no call)
  └─ report::render(&snapshot) → stdout
```

A typical repository with an upstream costs three Git subprocesses; one
without an upstream costs two. The status call is made exactly once and its
headers feed the branch and commit layers.

The commands run sequentially, so a Git operation running concurrently in
another tool can land between them. Each piece is internally consistent;
the snapshot as a whole is not atomic.

## How Git commands are executed

`git::commands::output` is the single spawn site (verified by the
`only_the_command_layer_starts_processes` test). It never uses a shell,
never concatenates strings into a command line, and pins the working
directory with `-C`. `run` wraps it and turns a non-zero exit into
`GitError::CommandFailed { args, code, stderr }`. Callers that expect a
specific non-zero exit (for example `rev-parse --verify --quiet`, which
exits 1 for a missing ref) use `output` and interpret the code themselves.

Diffkit does not override the user's Git configuration. The two
environment variables it sets only enforce the read-only and offline
contract (see `ENGINEERING_RULES.md`).

## How models are shared

`git::models` types are public and free of display logic. The CLI consumes
them through `RepositorySnapshot`; integration tests assert on them
directly; later phases can call `git::snapshot::inspect` or the individual
layers. Missing information is represented by `Option` or a dedicated enum
variant, so callers cannot mistake "unavailable" for "zero".
