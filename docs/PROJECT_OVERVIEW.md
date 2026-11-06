# Diffkit — Project Overview

## What Diffkit is

Diffkit is a fast, lightweight, terminal-native Git repository inspector
written in Rust. It reads the real state of a local repository through the
installed `git` executable and explains it: which changes are staged, which
are not, which files are untracked or conflicted, which branch is checked
out, and how that branch relates to its locally known upstream.

## Problem it solves

Developers change repositories from many tools at once — the terminal, VS
Code, IntelliJ IDEA, GitHub Desktop, other clients. Diffkit observes the
repository's actual Git state regardless of which tool changed it. It does
not wrap or intercept Git, and it never needs GitHub authentication or API
access.

## Core principles

- **Git is the source of truth.** Repository discovery, status, branch, and
  upstream information all come from Git itself.
- **Read-only.** Diffkit never stages, commits, fetches, pushes, checks out,
  or otherwise changes a repository (see `ENGINEERING_RULES.md`).
- **Offline.** No network operation is ever started automatically.
- **Honest about uncertainty.** Upstream comparisons use locally available
  references and are labelled as possibly stale. Missing information is
  reported as unavailable, never as a zero.
- **Correct before fast.** Filenames are preserved byte-for-byte; staged and
  unstaged states are never merged.

## What Phase 1 delivers

- Repository discovery from any directory in a working tree, including
  linked worktrees: root, Git directory, common Git directory, worktree kind,
  shallowness.
- A byte-level parser for `git status --porcelain=v2 -z --branch` covering
  ordinary, rename/copy, unmerged, untracked, ignored, and header records.
- Branch intelligence: normal branch, detached HEAD, unborn branch (and
  whether the repository is empty), HEAD commit.
- Upstream intelligence: not configured, not applicable (detached), missing
  tracking ref, or available with its commit and full ref name.
- Ahead/behind counts against the locally known upstream.
- One typed `RepositorySnapshot` combining all of the above.
- A diagnostic CLI: `diffkit`, `diffkit inspect`, `diffkit --help`,
  `diffkit --version`.

## Deliberately out of scope for Phase 1

Diff rendering of any kind, hunk parsing, syntax highlighting, a terminal UI,
keyboard navigation, filesystem watching or automatic refresh, caching,
commit browsing, GitHub/GitLab integration, authentication, any fetch, push,
staging, commit, reset, checkout, merge, or rebase operation, databases,
daemons, and cloud synchronization. See `docs/phases/` for where later work
belongs.
