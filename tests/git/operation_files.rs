//! Operations in progress detected from the state files Git writes in the
//! Git directory.

use diffkit::{git::state::*, types::repository::*};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn dir(label: &str) -> Dir {
    let d = std::env::temp_dir().join(format!("diffkit-state-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    Dir(d)
}

fn write(d: &Path, rel: &str, text: &str) {
    let p = d.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

#[test]
fn nothing_in_progress() {
    let d = dir("none");
    assert!(detect(&d.0).is_empty());
    assert!(detect(Path::new("/nonexistent/diffkit/git-dir")).is_empty());
}

#[test]
fn rebase_with_step_branch_and_onto() {
    let d = dir("rebase");
    let onto = "a".repeat(40);
    write(&d.0, "rebase-merge/interactive", "");
    write(&d.0, "rebase-merge/msgnum", "2\n");
    write(&d.0, "rebase-merge/end", "5\n");
    write(&d.0, "rebase-merge/head-name", "refs/heads/topic\n");
    write(&d.0, "rebase-merge/onto", &format!("{onto}\n"));
    let ops = detect(&d.0);
    assert_eq!(
        ops,
        vec![Operation::Rebase {
            backend: RebaseBackend::Merge,
            step: Some((2, 5)),
            branch: Some("refs/heads/topic".into()),
            onto: ObjectId::parse(onto.as_bytes()),
        }]
    );
    assert_eq!(ops[0].label(), "REBASING 2/5");
    assert!(ops[0]
        .description()
        .contains("on topic onto aaaaaaaaaaaa, step 2 of 5"));
}

#[test]
fn apply_backend_and_git_am_are_distinguished() {
    let d = dir("apply");
    write(&d.0, "rebase-apply/next", "1");
    write(&d.0, "rebase-apply/last", "3");
    write(&d.0, "rebase-apply/head-name", "detached HEAD");
    let ops = detect(&d.0);
    assert!(matches!(
        &ops[..],
        [Operation::Rebase {
            backend: RebaseBackend::Apply,
            step: Some((1, 3)),
            branch: None,
            ..
        }]
    ));
    write(&d.0, "rebase-apply/applying", "");
    assert_eq!(
        detect(&d.0),
        vec![Operation::ApplyMailbox { step: Some((1, 3)) }]
    );
}

#[test]
fn merge_cherry_pick_revert_bisect_and_sequencer() {
    let d = dir("ops");
    write(&d.0, "MERGE_HEAD", "x");
    write(&d.0, "REVERT_HEAD", "x");
    write(&d.0, "BISECT_LOG", "");
    write(&d.0, "BISECT_START", "main\n");
    write(&d.0, "sequencer/todo", "pick 1234 subject\n");
    assert_eq!(
        detect(&d.0),
        vec![
            Operation::Merge,
            Operation::CherryPick,
            Operation::Revert,
            Operation::Bisect {
                start: Some("main".into())
            },
        ]
    );
}

#[test]
fn oversized_or_garbage_state_files_leave_details_unknown() {
    let d = dir("garbage");
    write(&d.0, "rebase-merge/msgnum", &"9".repeat(5000));
    write(&d.0, "rebase-merge/end", "x");
    write(&d.0, "rebase-merge/onto", "not an id");
    assert!(matches!(
        &detect(&d.0)[..],
        [Operation::Rebase {
            step: None,
            onto: None,
            ..
        }]
    ));
}
