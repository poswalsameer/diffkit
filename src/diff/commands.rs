//! Argument arrays for the Git commands the diff engine runs. Execution
//! goes through [`crate::git::commands`], the only place processes start.
//!
//! The plumbing commands are used: `git diff-index --cached <base>` (staged),
//! `git diff-files` (unstaged), `git diff-index <base>` (combined), and
//! `git diff-tree -r <parent> <commit>` (commit inspection).
//! Porcelain `git diff` refreshes and rewrites `.git/index` after a
//! working-tree comparison even with `GIT_OPTIONAL_LOCKS=0`; the plumbing
//! commands never write. They do list files whose stat data changed but whose
//! content did not; discovery drops those (see `discovery`).
//!
//! The fixed options make output independent of user configuration that
//! would change its shape or run programs:
//!
//! | Option | Neutralizes |
//! |---|---|
//! | `--no-ext-diff` | `diff.external`, `GIT_EXTERNAL_DIFF`, `diff.<driver>.command` |
//! | `--no-textconv` | `diff.<driver>.textconv` filters |
//! | `--no-color` | `color.diff`, `color.ui` |
//! | `--no-relative` | `diff.relative` |
//! | `--find-renames --find-copies` | `diff.renames` (detection is always on) |
//! | `--diff-algorithm=histogram` | `diff.algorithm` |
//! | `--src-prefix=a/ --dst-prefix=b/` | `diff.noprefix`, `diff.mnemonicPrefix` |
//! | `--submodule=short` | `diff.submodule` |
//! | `-U<n> --inter-hunk-context=0` | `diff.context`, `diff.interHunkContext` |
//! | `--full-index` / `--no-abbrev` | `core.abbrev` (full object IDs) |
//!
//! Pathspecs are passed after `--` and are literal because the command layer
//! sets `GIT_LITERAL_PATHSPECS=1`.

use super::models::DiffComparison;
use crate::git::models::ObjectId;
use std::{ffi::OsString, path::Path};

const COMMON: [&str; 8] = [
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "--no-relative",
    "--find-renames",
    "--find-copies",
    "--diff-algorithm=histogram",
    "--submodule=short",
];

/// The plumbing command for a comparison, then the shared options.
fn command(comparison: DiffComparison) -> Vec<OsString> {
    let mut args: Vec<OsString> = match comparison {
        DiffComparison::Staged => vec!["diff-index".into(), "--cached".into()],
        DiffComparison::Unstaged | DiffComparison::Untracked => vec!["diff-files".into()],
        DiffComparison::Combined => vec!["diff-index".into()],
        DiffComparison::Commit => vec!["diff-tree".into(), "-r".into()],
    };
    args.extend(COMMON.iter().map(OsString::from));
    args
}

/// The trees compared. `diff-index` takes the base tree-ish after its
/// options; `diff-tree` takes the base and then the target commit.
#[derive(Debug, Clone, Copy)]
pub struct Range<'a> {
    pub base: &'a ObjectId,
    /// The commit inspected ([`DiffComparison::Commit`] only).
    pub target: Option<&'a ObjectId>,
}

fn range_args(comparison: DiffComparison, range: Range, args: &mut Vec<OsString>) {
    match comparison {
        DiffComparison::Staged | DiffComparison::Combined => args.push(range.base.as_str().into()),
        DiffComparison::Commit => {
            args.push(range.base.as_str().into());
            if let Some(t) = range.target {
                args.push(t.as_str().into());
            }
        }
        DiffComparison::Unstaged | DiffComparison::Untracked => {}
    }
}

/// Metadata for every changed file: one `--raw` record (modes, object IDs,
/// status, paths) and one `--numstat` record (line counts or binary) each.
pub fn discovery(comparison: DiffComparison, range: Range) -> Vec<OsString> {
    let mut args = command(comparison);
    args.extend(["--raw", "--numstat", "-z", "--no-abbrev"].map(OsString::from));
    range_args(comparison, range, &mut args);
    args
}

/// [`discovery`] limited to `paths` (literal pathspecs after `--`).
pub fn discovery_of(comparison: DiffComparison, range: Range, paths: &[&Path]) -> Vec<OsString> {
    let mut args = discovery(comparison, range);
    args.push("--".into());
    args.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    args
}

/// Unified patch for the given paths (all changed paths if empty).
pub fn patch(
    comparison: DiffComparison,
    range: Range,
    context: u32,
    paths: &[&Path],
) -> Vec<OsString> {
    let mut args = command(comparison);
    args.extend(
        [
            "--patch",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--inter-hunk-context=0",
        ]
        .map(OsString::from),
    );
    args.push(format!("-U{context}").into());
    range_args(comparison, range, &mut args);
    args.push("--".into());
    args.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    args
}

/// Untracked files Git does not ignore, NUL-terminated, relative to the root.
pub const UNTRACKED: [&str; 4] = ["ls-files", "--others", "--exclude-standard", "-z"];

pub fn blob_size(oid: &ObjectId) -> [OsString; 3] {
    ["cat-file".into(), "-s".into(), oid.as_str().into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid() -> ObjectId {
        ObjectId::parse(&[b'a'; 40]).unwrap()
    }

    fn head(base: &ObjectId) -> Range<'_> {
        Range { base, target: None }
    }

    #[test]
    fn comparisons_select_distinct_sides() {
        let o = oid();
        let staged = discovery(DiffComparison::Staged, head(&o));
        let unstaged = discovery(DiffComparison::Unstaged, head(&o));
        let combined = discovery(DiffComparison::Combined, head(&o));
        let base = OsString::from("a".repeat(40));
        assert_eq!(staged[..2], ["diff-index", "--cached"].map(OsString::from));
        assert_eq!(staged.last().unwrap(), &base);
        assert_eq!(unstaged[0], "diff-files");
        assert!(!unstaged.contains(&base));
        assert_eq!(combined[0], "diff-index");
        assert!(!combined.contains(&OsString::from("--cached")));
        assert_eq!(combined.last().unwrap(), &base);
    }

    #[test]
    fn patch_paths_follow_double_dash() {
        let o = oid();
        let a = patch(
            DiffComparison::Unstaged,
            head(&o),
            5,
            &[Path::new("-weird name"), Path::new(":(glob)*")],
        );
        let dd = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(
            a[dd + 1..],
            [OsString::from("-weird name"), OsString::from(":(glob)*")]
        );
        assert!(a.contains(&OsString::from("-U5")));
        for opt in [
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
            "--diff-algorithm=histogram",
        ] {
            assert!(a.contains(&OsString::from(opt)), "{opt}");
        }
    }

    #[test]
    fn commit_inspection_compares_parent_and_commit_trees() {
        let (p, c) = (oid(), ObjectId::parse(&[b'b'; 40]).unwrap());
        let range = Range {
            base: &p,
            target: Some(&c),
        };
        let d = discovery(DiffComparison::Commit, range);
        assert_eq!(d[..2], ["diff-tree", "-r"].map(OsString::from));
        assert_eq!(
            d[d.len() - 2..],
            [
                OsString::from("a".repeat(40)),
                OsString::from("b".repeat(40))
            ]
        );
        let pt = patch(DiffComparison::Commit, range, 3, &[Path::new("f")]);
        let dd = pt.iter().position(|x| x == "--").unwrap();
        assert_eq!(pt[dd - 1], OsString::from("b".repeat(40)));
        assert!(pt.contains(&OsString::from("--no-textconv")));
    }
}
