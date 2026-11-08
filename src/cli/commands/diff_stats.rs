//! `diffkit diff-stats [--load]`: a plain-text diagnostic of the diff
//! engine. It lists the changed files of every comparison with Git's line
//! counts and, with `--load`, loads every diff and reports hunk and row-kind
//! counts. It does not render diffs.

use crate::{
    diff::{engine::DiffEngine, errors::DiffError},
    types::diff::{
        DiffComparison, DiffContent, DiffFile, DiffFileList, DiffOptions, DiffRowKind,
        FileChangeKind, FileDiff,
    },
};
use crate::{git::snapshot, helpers::display::display_path};
use std::{fmt::Write, path::PathBuf};

/// Lists the changed files of every comparison for the repository
/// containing the current directory; with `load`, loads every diff too.
pub fn run(load: bool) -> i32 {
    let result = snapshot::inspect_current_dir()
        .map_err(DiffError::from)
        .map(|s| DiffEngine::from_snapshot(&s, DiffOptions::default()))
        .and_then(|engine| render(&engine, load));
    super::super::finish(result)
}

/// The loaded results of one list, summarized as each file loads so the
/// diffs themselves are never all held at once.
pub struct Loaded {
    /// One line per file, in list order.
    details: Vec<String>,
    unexpected_paths: Vec<PathBuf>,
}

impl Loaded {
    fn load(engine: &DiffEngine, list: &DiffFileList) -> Result<Self, DiffError> {
        let mut details = Vec::with_capacity(list.len());
        let unexpected_paths = engine.load_each(list, |_, result| {
            details.push(match result {
                Ok(diff) => loaded_detail(&diff),
                Err(e) => format!("error: {e}"),
            })
        })?;
        Ok(Self {
            details,
            unexpected_paths,
        })
    }
}

pub fn render(engine: &DiffEngine, load: bool) -> Result<String, DiffError> {
    let mut out = String::from("DIFFKIT - DIFF SUMMARY\n");
    let _ = writeln!(
        out,
        "Base: {}",
        match engine.head() {
            Some(h) => format!("HEAD {h}"),
            None => "empty tree (HEAD has no commits)".into(),
        }
    );
    for comparison in DiffComparison::ALL {
        let list = engine.discover(comparison)?;
        let loaded = if load {
            Some(Loaded::load(engine, &list)?)
        } else {
            None
        };
        section(&mut out, &list, loaded.as_ref());
    }
    Ok(out)
}

fn title(c: DiffComparison) -> &'static str {
    match c {
        DiffComparison::Staged => "STAGED",
        DiffComparison::Unstaged => "UNSTAGED",
        DiffComparison::Combined => "COMBINED",
        DiffComparison::Untracked => "UNTRACKED",
        DiffComparison::Commit => "COMMIT",
    }
}

/// One comparison's file list (and, when loaded, per-file results).
/// Shared with `diffkit show`.
pub fn section(out: &mut String, list: &DiffFileList, loaded: Option<&Loaded>) {
    let stats = list.line_stats();
    let _ = write!(
        out,
        "\n{} ({}): {} file{}",
        title(list.comparison),
        list.comparison.description(),
        list.len(),
        if list.len() == 1 { "" } else { "s" }
    );
    if list.comparison != DiffComparison::Untracked {
        let _ = write!(
            out,
            ", +{} -{}, {} binary",
            stats.added,
            stats.removed,
            list.binary_count()
        );
    }
    out.push('\n');
    for (i, f) in list.files.iter().enumerate() {
        let _ = write!(out, "  {:<3} {}", code(f), paths(f));
        if let Some(d) = detail(f) {
            let _ = write!(out, "  {d}");
        }
        if let Some(c) = loaded {
            let _ = write!(out, "  | {}", c.details[i]);
        }
        out.push('\n');
    }
    if let Some(c) = loaded {
        for p in &c.unexpected_paths {
            let _ = writeln!(out, "  !   {}  (changed after discovery)", display_path(p));
        }
    }
}

fn code(f: &DiffFile) -> &'static str {
    if f.comparison == DiffComparison::Untracked {
        return "??";
    }
    match f.kind {
        FileChangeKind::Added => "A",
        FileChangeKind::Modified => "M",
        FileChangeKind::Deleted => "D",
        FileChangeKind::Renamed => "R",
        FileChangeKind::Copied => "C",
        FileChangeKind::TypeChanged => "T",
        FileChangeKind::ModeChanged => "X",
        FileChangeKind::Unmerged => "U",
        FileChangeKind::Unknown(_) => "?",
    }
}

fn paths(f: &DiffFile) -> String {
    match (&f.old_path, &f.new_path) {
        (Some(o), Some(n)) if o != n => format!("{} -> {}", display_path(o), display_path(n)),
        _ => display_path(f.path()),
    }
}

fn detail(f: &DiffFile) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(s) = f.similarity {
        parts.push(format!("{s}% similar"));
    }
    if let (Some(o), Some(n)) = (f.old_mode, f.new_mode) {
        if o != n {
            parts.push(format!("mode {:o} -> {:o}", o.0, n.0));
        }
    }
    if f.kind == FileChangeKind::Unmerged {
        parts.push("unmerged".into());
    } else if f.conflicted {
        parts.push("conflicted".into());
    }
    match (f.binary, f.stats) {
        (crate::types::diff::BinaryStatus::Binary, _) => parts.push("binary".into()),
        (_, Some(s)) => parts.push(format!("+{} -{}", s.added, s.removed)),
        _ => {}
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn loaded_detail(d: &FileDiff) -> String {
    match &d.content {
        DiffContent::Text { hunks } => {
            let mut counts = [0usize; 4];
            for row in d.rows() {
                let i = match row.kind {
                    DiffRowKind::Context => 0,
                    DiffRowKind::Added => 1,
                    DiffRowKind::Removed => 2,
                    DiffRowKind::Modified => 3,
                    DiffRowKind::Empty => continue,
                };
                counts[i] += 1;
            }
            format!(
                "{} hunk{}; rows: {} context, {} added, {} removed, {} modified",
                hunks.len(),
                if hunks.len() == 1 { "" } else { "s" },
                counts[0],
                counts[1],
                counts[2],
                counts[3]
            )
        }
        DiffContent::Binary(b) => format!(
            "binary; size {} -> {}",
            b.old_size.map_or("-".into(), |s| s.to_string()),
            b.new_size.map_or("-".into(), |s| s.to_string())
        ),
        DiffContent::Submodule(s) => format!(
            "submodule {} -> {}{}",
            s.old_commit.as_ref().map_or("-", |c| c.as_str()),
            s.new_commit.as_ref().map_or("working tree", |c| c.as_str()),
            if s.new_dirty { " (dirty)" } else { "" }
        ),
        DiffContent::Unmerged(c) => match c.kind {
            Some(k) => format!("unmerged ({}): no two-way diff", k.description()),
            None => "unmerged: no two-way diff".into(),
        },
        DiffContent::TooLarge { limit, .. } => format!("too large (limit {limit} bytes)"),
        DiffContent::Unreadable { reason } => format!("unreadable: {reason}"),
        DiffContent::NestedRepository => "nested repository".into(),
    }
}
