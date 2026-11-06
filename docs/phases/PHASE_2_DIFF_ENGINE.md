# Phase 2 — Diff Engine (placeholder)

Not implemented. Not started.

Planned scope, pending approval: compute and parse diffs for the changes
that Phase 1 identifies (staged: index vs HEAD; unstaged: worktree vs
index; untracked: whole-file additions), including hunk parsing and
line-level models suitable for side-by-side presentation. It will consume
the Phase 1 `RepositorySnapshot` (paths, original paths, XY state, file
kind) rather than re-deriving repository state.
