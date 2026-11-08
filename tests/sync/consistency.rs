//! Reconciliation while Git changes the index, HEAD, and refs underneath
//! it: every settled result must describe one repository state, never a
//! status from before a change combined with lists from after it.

use crate::support;

use diffkit::{
    sync::{
        events::ChangeSet,
        reconcile::{self, Reconciled},
    },
    types::{
        diff::{DiffComparison, DiffOptions},
        repository::RepositorySnapshot,
    },
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use support::{Repo, TempDir};

fn status_paths(
    s: &RepositorySnapshot,
    pick: fn(&diffkit::types::repository::FileStatus) -> bool,
) -> BTreeSet<PathBuf> {
    s.status
        .files
        .iter()
        .filter(|f| pick(f))
        .map(|f| f.path.clone())
        .collect()
}

fn list_paths(r: &Reconciled, c: DiffComparison) -> BTreeSet<PathBuf> {
    let i = DiffComparison::ALL.iter().position(|&x| x == c).unwrap();
    r.lists[i]
        .as_ref()
        .unwrap()
        .files
        .iter()
        .map(|f| f.path().to_path_buf())
        .collect()
}

/// The snapshot (one `git status`) and the lists (separate commands) agree.
fn consistent(r: &Reconciled) -> Result<(), String> {
    let s = &r.snapshot;
    let checks = [
        (
            "staged",
            status_paths(s, |f| f.is_staged()),
            list_paths(r, DiffComparison::Staged),
        ),
        (
            "unstaged",
            status_paths(s, |f| f.is_unstaged()),
            list_paths(r, DiffComparison::Unstaged),
        ),
        (
            "untracked",
            status_paths(s, |f| f.is_untracked()),
            list_paths(r, DiffComparison::Untracked),
        ),
    ];
    for (what, status, list) in checks {
        if status != list {
            return Err(format!("{what}: status {status:?} vs list {list:?}"));
        }
    }
    let base = r.lists[0].as_ref().unwrap().base.clone();
    let head = s
        .branch
        .head_commit
        .clone()
        .unwrap_or_else(|| s.repository.object_format.empty_tree());
    if base != Some(head) {
        return Err("staged list compared against another HEAD".into());
    }
    Ok(())
}

#[test]
fn settled_results_never_mix_two_repository_states() {
    let tmp = TempDir::new("consistency");
    let repo = Repo::init(tmp.join("repo"));
    for i in 0..20 {
        repo.write(format!("f{i}.txt"), &format!("{i}\n"));
    }
    repo.commit_all("base");
    repo.write("f0.txt", "edited in the working tree\n");
    repo.write("untracked.txt", "u\n");
    // Blobs to stage without touching the working tree.
    let blobs: Vec<String> = (0..20)
        .map(|i| {
            let p = tmp.join(format!("blob{i}"));
            std::fs::write(&p, format!("staged {i}\n")).unwrap();
            repo.git(&[
                std::ffi::OsStr::new("hash-object"),
                std::ffi::OsStr::new("-w"),
                p.as_os_str(),
            ])
            .trim()
            .to_owned()
        })
        .collect();

    // Index, HEAD, and ref changes only: the working tree stays fixed, so
    // any disagreement comes from Git state moving during a reconciliation.
    let stop = Arc::new(AtomicBool::new(false));
    let hammer = {
        let stop = Arc::clone(&stop);
        let repo = repo.clone();
        std::thread::spawn(move || {
            let mut ops = 0;
            while !stop.load(Ordering::SeqCst) {
                let i = ops % 20;
                repo.git(&[
                    "update-index",
                    "--cacheinfo",
                    &format!("100644,{},f{i}.txt", blobs[i]),
                ]);
                repo.git(&["commit", "-q", "-m", "hammer"]);
                repo.git(&["reset", "-q", "--soft", "HEAD~1"]);
                repo.git(&["reset", "-q"]);
                ops += 1;
                // Bursts with short pauses, like a person or a tool
                // running Git commands.
                if ops % 3 == 0 {
                    std::thread::sleep(Duration::from_millis(60));
                }
            }
            ops
        })
    };
    let began = Instant::now();
    let (mut settled, mut unsettled, mut retries, mut mixed) = (0, 0, 0, 0);
    while began.elapsed() < Duration::from_secs(4) {
        let r = reconcile::reconcile(&repo.path, DiffOptions::default(), ChangeSet::MANUAL, None)
            .unwrap();
        retries += r.retries;
        if r.settled {
            settled += 1;
            if let Err(e) = consistent(&r) {
                stop.store(true, Ordering::SeqCst);
                panic!("a settled result mixed two states: {e}");
            }
        } else {
            unsettled += 1;
            mixed += usize::from(consistent(&r).is_err());
        }
    }
    stop.store(true, Ordering::SeqCst);
    let ops = hammer.join().unwrap();
    eprintln!(
        "hammer rounds {ops}; settled {settled}; unsettled {unsettled} \
         ({mixed} of them inconsistent); retries {retries}"
    );
    assert!(
        ops > 10,
        "Git changed state while reconciling ({ops} rounds)"
    );
    assert!(retries > 0, "changes during reconciliation were detected");
    assert!(
        settled > 0,
        "some reconciliations completed between changes"
    );
    // Quiet again: a fresh reconciliation is settled and consistent.
    let r =
        reconcile::reconcile(&repo.path, DiffOptions::default(), ChangeSet::MANUAL, None).unwrap();
    assert!(r.settled && r.retries == 0);
    consistent(&r).unwrap();
}
