//! Binary, symlink, empty, newline, CRLF, type-change, conflict, and
//! submodule diffs from real Git output.

mod common;

use common::{discover, engine, entry, load, row, rows, Repo, TempDir};
use diffkit::diff::{
    BinaryStatus, BinarySummary, DiffComparison::*, DiffContent, DiffRowKind::*, FileChangeKind,
    LineEnding,
};
use diffkit::git::models::ConflictKind;

fn repo(label: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new(label);
    let repo = Repo::init(tmp.join("repo"));
    (tmp, repo)
}

fn write_bytes(repo: &Repo, rel: &str, bytes: &[u8]) {
    std::fs::write(repo.path.join(rel), bytes).unwrap();
}

#[test]
fn binary_addition_modification_and_deletion() {
    let (_t, repo) = repo("diff-binary");
    const OLD: &[u8] = b"\x00\x01\x02 old";
    const DEL: &[u8] = b"\x00 to delete";
    const ADD: &[u8] = b"\x00\x00 new binary";
    const NEW: &[u8] = b"\x00\x01\x02 new and longer";
    let len = |b: &[u8]| Some(b.len() as u64);
    write_bytes(&repo, "mod.bin", OLD);
    write_bytes(&repo, "del.bin", DEL);
    repo.commit_all("base");
    write_bytes(&repo, "add.bin", ADD);
    repo.git(&["add", "add.bin"]);
    repo.git(&["rm", "-q", "del.bin"]);
    write_bytes(&repo, "mod.bin", NEW);
    let e = engine(&repo.path);

    let staged = discover(&e, Staged);
    for f in &staged.files {
        assert_eq!(f.binary, BinaryStatus::Binary, "{:?}", f.path());
        assert_eq!(f.stats, None, "no invented line counts");
    }
    assert_eq!(staged.binary_count(), 2);
    let add = e
        .load_file_diff(&staged, entry(&staged, "add.bin").id)
        .unwrap();
    assert_eq!(
        add.content,
        DiffContent::Binary(BinarySummary {
            old_size: None,
            new_size: len(ADD)
        })
    );
    assert!(add.hunks().is_empty());
    let del = e
        .load_file_diff(&staged, entry(&staged, "del.bin").id)
        .unwrap();
    assert_eq!(
        del.content,
        DiffContent::Binary(BinarySummary {
            old_size: len(DEL),
            new_size: None
        })
    );
    let m = load(&e, Unstaged, "mod.bin");
    assert_eq!(m.file.kind, FileChangeKind::Modified);
    assert_eq!(
        m.content,
        DiffContent::Binary(BinarySummary {
            old_size: len(OLD),
            new_size: len(NEW)
        })
    );
}

#[cfg(unix)]
#[test]
fn symlink_target_change() {
    let (_t, repo) = repo("diff-symlink");
    std::os::unix::fs::symlink("targets/first-target", repo.path.join("link")).unwrap();
    repo.commit_all("base");
    std::fs::remove_file(repo.path.join("link")).unwrap();
    std::os::unix::fs::symlink("targets/second-target", repo.path.join("link")).unwrap();
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "link");
    assert_eq!(d.file.new_mode.unwrap().0, 0o120000);
    assert_eq!(
        rows(&d),
        vec![row(
            Modified,
            Some(1),
            Some(1),
            "targets/first-target",
            "targets/second-target"
        )]
    );
    let new = d.rows().next().unwrap().new_line.as_ref().unwrap();
    assert_eq!(
        new.ending,
        LineEnding::None,
        "symlink targets have no newline"
    );
}

#[cfg(unix)]
#[test]
fn file_replaced_by_symlink_is_a_type_change() {
    let (_t, repo) = repo("diff-typechange");
    repo.write("t", "regular content\n");
    repo.commit_all("base");
    std::fs::remove_file(repo.path.join("t")).unwrap();
    std::os::unix::fs::symlink("somewhere", repo.path.join("t")).unwrap();
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "t");
    assert_eq!(d.file.kind, FileChangeKind::TypeChanged);
    assert_eq!(
        (d.file.old_mode.unwrap().0, d.file.new_mode.unwrap().0),
        (0o100644, 0o120000)
    );
    assert_eq!(d.hunks().len(), 1);
    assert_eq!(
        rows(&d),
        vec![
            row(Removed, Some(1), None, "regular content", ""),
            row(Added, None, Some(1), "", "somewhere")
        ]
    );
}

#[test]
fn empty_files() {
    let (_t, repo) = repo("diff-empty");
    repo.write("was-empty", "");
    repo.write("to-delete", "");
    repo.commit_all("base");
    repo.write("was-empty", "now has a line\n");
    repo.write("new-empty", "");
    repo.git(&["add", "-A"]);
    repo.git(&["rm", "-q", "to-delete"]);
    let e = engine(&repo.path);
    let list = discover(&e, Staged);
    let filled = e
        .load_file_diff(&list, entry(&list, "was-empty").id)
        .unwrap();
    assert_eq!(
        rows(&filled),
        vec![row(Added, None, Some(1), "", "now has a line")]
    );
    assert_eq!(filled.hunks()[0].header, b"@@ -0,0 +1 @@");
    for path in ["new-empty", "to-delete"] {
        let f = list
            .files
            .iter()
            .find(|f| f.path() == std::path::Path::new(path));
        if let Some(f) = f {
            assert_eq!(f.has_hunks(), Some(false), "{path}");
            assert!(e.load_file_diff(&list, f.id).unwrap().hunks().is_empty());
        }
    }
    // Git may pair the empty files as a copy/rename; either way both paths
    // are accounted for and no hunks are invented.
    let mentioned: Vec<_> = list
        .files
        .iter()
        .flat_map(|f| [f.old_path.clone(), f.new_path.clone()])
        .flatten()
        .collect();
    assert!(mentioned
        .iter()
        .any(|p| p == std::path::Path::new("new-empty")));
}

#[test]
fn missing_final_newline() {
    let (_t, repo) = repo("diff-no-eol");
    repo.write("f", "a\nb");
    repo.write("g", "x\ny");
    repo.commit_all("base");
    repo.write("f", "a\nc");
    repo.write("g", "x\ny\n");
    let e = engine(&repo.path);
    let f = load(&e, Unstaged, "f");
    let r: Vec<_> = f.rows().collect();
    assert_eq!(
        r[1].kind, Removed,
        "single letters are not similar enough to pair"
    );
    assert_eq!(r[1].old_line.as_ref().unwrap().ending, LineEnding::None);
    assert_eq!(r[2].new_line.as_ref().unwrap().ending, LineEnding::None);
    let g = load(&e, Unstaged, "g");
    let m = g
        .rows()
        .find(|r| r.kind == Modified)
        .expect("newline added at end");
    assert_eq!(m.old_line.as_ref().unwrap().content, b"y");
    assert_eq!(m.old_line.as_ref().unwrap().ending, LineEnding::None);
    assert_eq!(m.new_line.as_ref().unwrap().ending, LineEnding::Lf);
}

#[test]
fn crlf_and_mixed_line_endings() {
    let (_t, repo) = repo("diff-crlf");
    write_bytes(&repo, "w.txt", b"alpha line\r\nbeta line\r\ngamma line\n");
    repo.commit_all("base");
    write_bytes(
        &repo,
        "w.txt",
        b"alpha line\r\nbeta line changed\r\ngamma line\r\n",
    );
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "w.txt");
    let r: Vec<_> = d.rows().collect();
    assert_eq!(r[0].kind, Context);
    assert_eq!(r[0].old_line.as_ref().unwrap().ending, LineEnding::CrLf);
    let endings: Vec<_> = r
        .iter()
        .filter(|r| r.kind == Modified)
        .map(|r| {
            (
                r.old_line.as_ref().unwrap().ending,
                r.new_line.as_ref().unwrap().ending,
            )
        })
        .collect();
    assert_eq!(
        endings,
        vec![
            (LineEnding::CrLf, LineEnding::CrLf),
            (LineEnding::Lf, LineEnding::CrLf)
        ]
    );
    let gamma = r
        .iter()
        .find(|r| {
            r.old_line
                .as_ref()
                .is_some_and(|l| l.content == b"gamma line")
        })
        .unwrap();
    assert_eq!(
        gamma.kind, Modified,
        "same text with a new line ending is a modification"
    );
}

#[test]
fn non_utf8_content_is_kept_as_bytes() {
    let (_t, repo) = repo("diff-non-utf8-content");
    write_bytes(&repo, "latin1.txt", b"caf\xe9 au lait\nplain\n");
    repo.commit_all("base");
    write_bytes(&repo, "latin1.txt", b"caf\xe9 au lait \xff\xfe\nplain\n");
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "latin1.txt");
    assert!(
        matches!(d.content, DiffContent::Text { .. }),
        "non-UTF-8 is not binary"
    );
    let m = d.rows().next().unwrap();
    assert_eq!(m.kind, Modified);
    assert_eq!(m.old_line.as_ref().unwrap().content, b"caf\xe9 au lait");
    assert_eq!(
        m.new_line.as_ref().unwrap().content,
        b"caf\xe9 au lait \xff\xfe"
    );
}

fn conflicted_repo(label: &str) -> (TempDir, Repo) {
    let (t, repo) = repo(label);
    repo.write("c.txt", "base\n");
    repo.write("other.txt", "o\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "side"]);
    repo.write("c.txt", "side\n");
    repo.commit_all("side");
    repo.git(&["switch", "-q", "main"]);
    repo.write("c.txt", "main\n");
    repo.commit_all("main");
    assert!(!repo.git_output(&["merge", "side"]).status.success());
    repo.write("other.txt", "o\nedited during the merge\n");
    (t, repo)
}

#[test]
fn conflicts_are_explicit_in_staged_and_unstaged() {
    let (_t, repo) = conflicted_repo("diff-conflict");
    let e = engine(&repo.path);
    for c in [Staged, Unstaged] {
        let list = discover(&e, c);
        let f = entry(&list, "c.txt");
        assert_eq!(f.kind, FileChangeKind::Unmerged, "{c:?}");
        assert!(f.conflicted);
        assert_eq!(
            list.files
                .iter()
                .filter(|f| f.path() == std::path::Path::new("c.txt"))
                .count(),
            1
        );
        let d = e.load_file_diff(&list, f.id).unwrap();
        let DiffContent::Unmerged(info) = &d.content else {
            panic!("no invented two-way diff: {:?}", d.content);
        };
        assert_eq!(info.kind, Some(ConflictKind::BothModified));
        let stages = info.stages.as_ref().expect("stages from git status");
        assert!(stages.base.is_some() && stages.ours.is_some() && stages.theirs.is_some());
        assert!(info.worktree_present);
        let all = e.load_all(&list).unwrap();
        assert!(
            all.unexpected_paths.is_empty(),
            "{:?}",
            all.unexpected_paths
        );
        for l in &all.files {
            assert!(l.result.is_ok(), "{c:?}: {:?}", l.result);
        }
    }
    let other = load(&e, Unstaged, "other.txt");
    assert_eq!(
        rows(&other)[1],
        row(Added, None, Some(2), "", "edited during the merge")
    );
}

#[test]
fn combined_view_of_a_conflict_is_flagged() {
    let (_t, repo) = conflicted_repo("diff-conflict-combined");
    let e = engine(&repo.path);
    let list = discover(&e, Combined);
    let f = entry(&list, "c.txt");
    assert_eq!(
        f.kind,
        FileChangeKind::Modified,
        "Git compares HEAD with the file holding markers"
    );
    assert!(f.conflicted);
    let d = e.load_file_diff(&list, f.id).unwrap();
    let added: Vec<_> = rows(&d)
        .into_iter()
        .filter(|r| r.0 == Added)
        .map(|r| r.4)
        .collect();
    assert!(added.iter().any(|t| t.starts_with("<<<<<<<")), "{added:?}");
}

#[test]
fn submodule_changes_are_summarized() {
    let tmp = TempDir::new("diff-submodule");
    let sub_src = Repo::init(tmp.join("sub-src"));
    sub_src.write("s.txt", "s\n");
    sub_src.commit_all("s");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("r.txt", "r\n");
    repo.commit_all("r");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        sub_src.path.to_str().unwrap(),
        "sub",
    ]);
    repo.commit_all("add sub");
    let recorded = repo.rev("HEAD:sub");
    let sub = Repo {
        path: repo.path.join("sub"),
    };
    sub.git(&["config", "user.name", "T"]);
    sub.git(&["config", "user.email", "t@diffkit.invalid"]);
    sub.write("s.txt", "s2\n");
    let moved = sub.commit_all("move");
    sub.write("s.txt", "dirty\n");
    let e = engine(&repo.path);
    let d = load(&e, Unstaged, "sub");
    match &d.content {
        DiffContent::Submodule(s) => {
            assert_eq!(s.old_commit.as_ref().unwrap().as_str(), recorded);
            assert!(s.new_dirty);
            if let Some(n) = &s.new_commit {
                assert_eq!(n.as_str(), moved);
            }
        }
        other => panic!("{other:?}"),
    }
}
