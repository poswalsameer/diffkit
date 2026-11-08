//! Phase 5 release benchmark: scenarios earlier benchmarks did not cover.
//!
//! cargo run --release --example bench_release [-- RUNS]
//!
//! 1. 1,000 changed files (5,000 tracked): startup, first frame, first diff,
//!    manual refresh, and a walk over every file (cache hits, evictions at
//!    the default and a small budget).
//! 2. A burst of 1,000 writes under the real watcher: events, runs, latency.
//! 3. Three linked worktrees: startup of each, and how long a ref moved in
//!    one takes to appear in another (real watcher).
//! 4. History: listing 200 of 1,000 commits, opening a commit, loading its
//!    first file, all in the interface state.
//!
//! In-process, release build, medians of RUNS measured runs after one
//! warm-up. Fixture setup runs writing Git commands; Diffkit does not.

use diffkit::{
    git::commands::thread_invocations,
    sync::{
        events::ChangeSet,
        reconcile::{self, Reconciled},
        scheduler::{GitReconciler, SyncConfig, SyncService, SyncUpdate},
    },
    tui::{actions::Action, app::App, ui},
    types::diff::DiffOptions,
};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Bench")
        .env("GIT_AUTHOR_EMAIL", "bench@diffkit.invalid")
        .env("GIT_COMMITTER_NAME", "Bench")
        .env("GIT_COMMITTER_EMAIL", "bench@diffkit.invalid")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(repo: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn file_path(i: usize) -> String {
    format!("src/mod{:03}/file{i:06}.rs", i / 100)
}

fn content(i: usize) -> String {
    (0..20)
        .map(|l| format!("// file {i} line {l}\npub fn f{i}_{l}() -> usize {{ {l} }}\n"))
        .collect()
}

fn fresh(base: &Path, name: &str) -> PathBuf {
    let repo = base.join(name);
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    repo
}

fn settle_index(repo: &Path) {
    std::thread::sleep(Duration::from_millis(1100));
    git(repo, &["update-index", "-q", "--really-refresh"]);
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let s = Instant::now();
    let t = f();
    (t, ms(s.elapsed()))
}

fn frame(term: &mut Terminal<TestBackend>, app: &App) -> f64 {
    time(|| term.draw(|f| ui::render(f, app)).unwrap()).1
}

/// A real watcher session that records published results.
struct Watch {
    service: SyncService,
    rx: Receiver<diffkit::sync::scheduler::SyncEvent>,
    /// Diagnostics of the latest event received.
    diag: std::cell::Cell<diffkit::sync::events::SyncDiagnostics>,
}

impl Watch {
    fn start(path: &Path) -> Self {
        let opts = DiffOptions::default();
        let state = reconcile::reconcile(path, opts, ChangeSet::STARTUP, None).unwrap();
        let (tx, rx) = mpsc::channel();
        let service = SyncService::start(
            SyncConfig::default(),
            GitReconciler::new(path.to_path_buf(), opts, state),
            move |e| {
                let _ = tx.send(e);
            },
        );
        let w = Self {
            service,
            rx,
            diag: Default::default(),
        };
        w.until(|_| true);
        w
    }

    /// Waits for a published result satisfying `pred`; returns it, the
    /// time it arrived, and the diagnostics at that point.
    fn until(
        &self,
        pred: impl Fn(&Reconciled) -> bool,
    ) -> (
        Option<Reconciled>,
        Instant,
        diffkit::sync::events::SyncDiagnostics,
    ) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let e = self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("result within 60 s");
            self.diag.set(e.diagnostics);
            match e.update {
                SyncUpdate::Reconciled(r) if pred(&r) => {
                    return (Some(*r), Instant::now(), e.diagnostics)
                }
                SyncUpdate::Unchanged => return (None, Instant::now(), e.diagnostics),
                _ => {}
            }
        }
    }

    /// Drains until nothing arrives for 1.5 s; returns the latest
    /// diagnostics.
    fn settle(&self) -> diffkit::sync::events::SyncDiagnostics {
        while let Ok(e) = self.rx.recv_timeout(Duration::from_millis(1500)) {
            self.diag.set(e.diagnostics);
        }
        self.diag.get()
    }
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(5);
    let base = std::env::temp_dir().join("diffkit-bench-release");
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let size = Rect::new(0, 0, 200, 60);
    println!(
        "Release build, in-process; {runs} measured runs after 1 warm-up (medians); \
         frames rendered into a 200x60 TestBackend.\n"
    );

    // ------------------------------------------------- 1. 1,000 changed files
    let repo = fresh(&base, "changed-1000");
    for i in 0..5_000 {
        write(&repo, &file_path(i), content(i));
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    settle_index(&repo);
    for i in (0..5_000).step_by(5) {
        write(
            &repo,
            &file_path(i),
            content(i).replace("line 3\n", "line three\n"),
        );
    }
    let mut v: [Vec<f64>; 5] = Default::default();
    let mut calls = (0, 0, 0);
    let mut walk = (0.0, 0, 0, 0.0);
    let mut small = (0, 0);
    for run in 0..=runs {
        let mut term = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
        let c0 = thread_invocations();
        let (mut app, t_open) = time(|| App::open(&repo, size).unwrap());
        let c1 = thread_invocations();
        let t_frame = frame(&mut term, &app);
        let ((), t_load) = time(|| app.load_pending());
        let c2 = thread_invocations();
        let ((), t_refresh) = time(|| app.dispatch(Action::Refresh));
        let c3 = thread_invocations();
        // Walk all 1,000 files twice: the second pass is served by the cache.
        let pass = |app: &mut App| {
            app.dispatch(Action::SelectFirstFile);
            for _ in 0..999 {
                app.dispatch(Action::SelectNextFile);
            }
        };
        let walk_start = Instant::now();
        pass(&mut app);
        let first = app.cache_stats();
        pass(&mut app);
        let t_walk = ms(walk_start.elapsed());
        let s = app.cache_stats();
        let second_hits = (s.hits - first.hits) as f64
            / ((s.hits - first.hits) + (s.misses - first.misses)) as f64
            * 100.0;
        // The same walk with a 256 KiB budget.
        app.set_cache_budget(256 << 10);
        let before = app.cache_stats();
        for _ in 0..2 {
            app.dispatch(Action::SelectFirstFile);
            for _ in 0..999 {
                app.dispatch(Action::SelectNextFile);
            }
        }
        let after = app.cache_stats();
        if run > 0 {
            for (vec, t) in v
                .iter_mut()
                .zip([t_open, t_frame, t_load, t_refresh, t_walk])
            {
                vec.push(t);
            }
            calls = (c1 - c0, c2 - c1, c3 - c2);
            walk = (
                second_hits,
                s.entries,
                s.evictions,
                s.peak_bytes as f64 / 1048576.0,
            );
            small = (
                after.misses - before.misses,
                after.evictions - before.evictions,
            );
        }
    }
    let [open, frame1, load, refresh, walk_t] = v.map(median);
    println!("## 1,000 changed files (5,000 tracked)\n");
    println!("| Measure | Result |");
    println!("|---|---:|");
    println!(
        "| App::open (snapshot + 4 lists) | {open:.1} ms, {} Git |",
        calls.0
    );
    println!("| First frame | {frame1:.2} ms |");
    println!("| First diff load | {load:.2} ms, {} Git |", calls.1);
    println!(
        "| Manual refresh (inline, full) | {refresh:.1} ms, {} Git |",
        calls.2
    );
    println!("| Walk 1,000 files twice (first pass loads, second is cached) | {walk_t:.0} ms |");
    println!(
        "| Cache, 64 MiB budget | second pass {:.1}% hits; {} entries, {} evictions, peak {:.2} MiB |",
        walk.0, walk.1, walk.2, walk.3
    );
    println!(
        "| Same walk, 256 KiB budget | {} misses, {} evictions |\n",
        small.0, small.1
    );

    // ----------------------------------------------------- 2. burst of writes
    let w = Watch::start(&repo);
    let mut latencies = Vec::new();
    let mut runs_per_burst = Vec::new();
    let mut events = Vec::new();
    for run in 0..=runs {
        let d0 = w.settle();
        for k in 0..1000 {
            let i = (k * 5) % 5_000;
            write(
                &repo,
                &file_path(i),
                content(i).replace("line 4\n", &format!("line four {run}\n")),
            );
        }
        let end = Instant::now();
        let (_, at, _) = w.until(|_| true);
        let d1 = w.settle();
        if run > 0 {
            latencies.push(ms(at - end));
            runs_per_burst.push((d1.reconciles_completed - d0.reconciles_completed) as f64);
            events.push((d1.fs_events - d0.fs_events) as f64);
        }
    }
    assert!(w.service.shutdown(Duration::from_secs(5)));
    println!("## Burst: 1,000 writes to 1,000 files, real watcher\n");
    println!(
        "Median: {:.0} watcher events, {:.0} reconciliations, first result {:.0} ms after the last write.\n",
        median(events),
        median(runs_per_burst),
        median(latencies)
    );

    // --------------------------------------------- 3. linked worktrees
    let main_repo = fresh(&base, "worktrees");
    for i in 0..5_000 {
        write(&main_repo, &file_path(i), content(i));
    }
    git(&main_repo, &["add", "-A"]);
    git(&main_repo, &["commit", "-q", "-m", "fixture"]);
    let mut wts = Vec::new();
    for n in 0..3 {
        let p = base.join(format!("worktrees-wt{n}"));
        let _ = std::fs::remove_dir_all(&p);
        let _ = git(&main_repo, &["worktree", "prune"]);
        git(
            &main_repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("wt{n}"),
                p.to_str().unwrap(),
            ],
        );
        git(&p, &["branch", "-q", "--set-upstream-to=main"]);
        wts.push(p);
    }
    settle_index(&main_repo);
    for p in &wts {
        settle_index(p);
    }
    let mut opens = Vec::new();
    let mut open_calls = 0;
    for run in 0..=runs {
        for p in &wts {
            let c0 = thread_invocations();
            let (_app, t) = time(|| App::open(p, size).unwrap());
            if run > 0 {
                opens.push(t);
                open_calls = thread_invocations() - c0;
            }
        }
    }
    let w = Watch::start(&wts[0]);
    let mut shared = Vec::new();
    for run in 0..=runs {
        w.settle();
        git(
            &wts[1],
            &["commit", "-q", "--allow-empty", "-m", &format!("r{run}")],
        );
        // Move the shared `main` ref, as a fetch or another worktree would.
        git(&wts[1], &["update-ref", "refs/heads/main", "HEAD"]);
        let end = Instant::now();
        let (_, at, _) = w.until(|r| {
            matches!(
                r.snapshot.commit_tracking,
                diffkit::types::repository::CommitTrackingInfo::Compared { behind, .. } if behind > 0
            )
        });
        if run > 0 {
            shared.push(ms(at - end));
        }
        git(&wts[1], &["update-ref", "refs/heads/main", "HEAD~1"]);
        w.until(|_| true);
    }
    assert!(w.service.shutdown(Duration::from_secs(5)));
    println!("## Three linked worktrees (5,000 tracked files)\n");
    println!(
        "App::open of a linked worktree: {:.1} ms median over all three, {open_calls} Git.",
        median(opens)
    );
    println!(
        "A commit published to `main` from worktree 2 appears as \"behind\" in worktree 1: {:.0} ms median after the ref moved.\n",
        median(shared)
    );

    // ------------------------------------------------------------ 4. history
    let hist = fresh(&base, "history");
    write(&hist, "base.txt", "base\n");
    git(&hist, &["add", "-A"]);
    git(&hist, &["commit", "-q", "-m", "base"]);
    // 1,000 commits through fast-import (setup only).
    let mut stream = String::new();
    for i in 0..1000 {
        let body = content(i);
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter Bench <b@diffkit.invalid> {} +0000\ndata {}\ncommit {i}\n{}M 100644 inline src/f{:03}.rs\ndata {}\n{body}\n",
            1_700_000_000 + i,
            format!("commit {i}\n").len(),
            if i == 0 { "from refs/heads/main^0\n" } else { "" },
            i % 50,
            body.len()
        ));
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&hist)
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    git(&hist, &["reset", "-q", "--hard"]);
    let mut v: [Vec<f64>; 3] = Default::default();
    let mut c = (0, 0, 0);
    for run in 0..=runs {
        let mut app = App::open(&hist, size).unwrap();
        app.load_pending();
        let c0 = thread_invocations();
        let ((), t_list) = time(|| app.dispatch(Action::ToggleHistory));
        let c1 = thread_invocations();
        app.dispatch(Action::SelectNextFile);
        let ((), t_open) = time(|| app.apply(Action::OpenSelected));
        let c2 = thread_invocations();
        let ((), t_load) = time(|| app.load_pending());
        let c3 = thread_invocations();
        if run > 0 {
            v[0].push(t_list);
            v[1].push(t_open);
            v[2].push(t_load);
            c = (c1 - c0, c2 - c1, c3 - c2);
        }
    }
    let [list, open_c, load_c] = v.map(median);
    println!("## History (1,001 commits)\n");
    println!("| Measure | Result |");
    println!("|---|---:|");
    println!(
        "| List 200 recent commits (L) | {list:.1} ms, {} Git |",
        c.0
    );
    println!(
        "| Open a commit (its file list) | {open_c:.1} ms, {} Git |",
        c.1
    );
    println!("| Load its first file | {load_c:.1} ms, {} Git |", c.2);
}
