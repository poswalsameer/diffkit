//! `diffkit show <commit>`: plain-text metadata of one local commit and the
//! files it changed (against its first parent), with Git's line counts.
//! Nothing is fetched; only the local object database is read.

use super::diff_stats;
use crate::git::snapshot;
use crate::{
    diff::{engine::DiffEngine, errors::DiffError},
    git::history,
    helpers::display::escape_text,
    types::{diff::DiffOptions, repository::RepositorySnapshot},
};
use std::{ffi::OsStr, fmt::Write};

/// Prints a local commit of the repository containing the current
/// directory. The original `OsStr` is used: a ref name may not be UTF-8.
pub fn run(rev: &OsStr) -> i32 {
    super::super::finish(
        snapshot::inspect_current_dir()
            .map_err(DiffError::from)
            .and_then(|s| render(&s, rev)),
    )
}

pub fn render(snapshot: &RepositorySnapshot, rev: &OsStr) -> Result<String, DiffError> {
    let repo = &snapshot.repository;
    let id = history::resolve_commit(&repo.root, rev)?;
    let shallow = repo.common_git_dir.join("shallow");
    let c = history::commit(&repo.root, &id, &shallow)?;
    let mut out = String::from("DIFFKIT - COMMIT\n");
    let _ = writeln!(out, "Commit: {}", c.id);
    let _ = writeln!(out, "Short: {}", escape_text(&c.short_id));
    let parents = if c.parents.is_empty() {
        if c.shallow_boundary {
            "not available (shallow clone boundary)".to_string()
        } else {
            "none (root commit)".to_string()
        }
    } else {
        c.parents
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let _ = writeln!(out, "Parents: {parents}");
    for (label, s) in [("Author", &c.author), ("Committer", &c.committer)] {
        let _ = writeln!(
            out,
            "{label}: {} <{}> {}",
            escape_text(&s.name),
            escape_text(&s.email),
            escape_text(&s.date)
        );
    }
    out.push('\n');
    for line in c.message.lines() {
        let _ = writeln!(out, "    {}", escape_text(line));
    }
    if c.shallow_boundary && c.parents.is_empty() {
        let _ = writeln!(
            out,
            "\nFiles: unavailable; the parent of this commit is not in this shallow clone."
        );
        return Ok(out);
    }
    let engine = DiffEngine::from_snapshot(snapshot, DiffOptions::default());
    let list = engine.discover_commit(&c.id, c.first_parent())?;
    let _ = write!(
        out,
        "\nCompared with: {}",
        match c.first_parent() {
            Some(p) if c.is_merge() => format!("first parent {p} (merge commit)"),
            Some(p) => format!("parent {p}"),
            None => "the empty tree (root commit)".into(),
        }
    );
    out.push('\n');
    diff_stats::section(&mut out, &list, None);
    Ok(out)
}
