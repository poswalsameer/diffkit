//! Phase 4 synchronization benchmark.
//!
//! cargo run --release --example bench_sync [-- RUNS]
//!
//! Uses the real watcher (`notify`), debouncer (200 ms quiet, 1 s maximum),
//! and Git reconciler on generated repositories of 100, 5,000, and 50,000
//! tracked files. For each scenario it records, per run: events received,
//! reconciliations, Git processes started by the worker, reconciliation
//! time, and the latency from the end of the external change to the
//! published state (this includes the debounce quiet period). Medians over
//! RUNS runs are printed, after one warm-up run per scenario.
//!
//! Cache scenarios run in-process on the interface state.
//!
//! Fixture changes are made with ordinary file writes and Git commands
//! (add, switch, checkout, clean); those are the benchmark's, never
//! Diffkit's.

use diffkit::{
    diff::{DiffComparison, DiffOptions},
    git::commands::thread_invocations,
    sync::{
        reconcile, ChangeSet, GitReconciler, Reconciled, SyncConfig, SyncDiagnostics, SyncEvent,
        SyncService, SyncUpdate,
    },
    tui::{Action, App},
};
use ratatui::layout::Rect;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

fn git(dir: &Path, args: &[&str]) {
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn file_path(i: usize) -> String {
    format!("src/mod{:03}/file{i:06}.rs", i / 100)
}

fn content(i: usize) -> String {
    (0..20)
        .map(|l| format!("// file {i} line {l}\npub fn f{i}_{l}() -> usize {{ {l} }}\n"))
        .collect()
}

fn write(repo: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn build(base: &Path, tracked: usize) -> PathBuf {
    let repo = base.join(format!("repo-{tracked}"));
    if repo.join(".git/bench-ready").exists() {
        return repo;
    }
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for i in 0..tracked {
        write(&repo, &file_path(i), content(i));
    }
    write(&repo, ".gitignore", "target/\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    std::thread::sleep(Duration::from_millis(1100));
    git(&repo, &["update-index", "-q", "--really-refresh"]);
    std::fs::write(repo.join(".git/bench-ready"), "").unwrap();
    repo
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return f64::NAN;
    }
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

struct Session {
    service: SyncService,
    rx: Receiver<(Instant, SyncEvent)>,
    state: Reconciled,
    diag: SyncDiagnostics,
}

impl Session {
    fn start(path: &Path) -> Self {
        let opts = DiffOptions::default();
        let state = reconcile::reconcile(path, opts, ChangeSet::STARTUP, None).unwrap();
        let (tx, rx) = mpsc::channel();
        let service = SyncService::start(
            SyncConfig::default(),
            GitReconciler::new(path.to_path_buf(), opts, state.clone()),
            move |e| {
                let _ = tx.send((Instant::now(), e));
            },
        );
        let mut s = Self {
            service,
            rx,
            state,
            diag: SyncDiagnostics::default(),
        };
        s.until(|_, d| d.reconciles_completed >= 1, Duration::from_secs(60));
        s.settle();
        s
    }

    /// Waits until `pred` holds; returns when the satisfying update arrived.
    fn until(
        &mut self,
        pred: impl Fn(&Reconciled, &SyncDiagnostics) -> bool,
        timeout: Duration,
    ) -> Instant {
        let deadline = Instant::now() + timeout;
        let mut at = Instant::now();
        while !pred(&self.state, &self.diag) {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out");
            let (t, e) = self.rx.recv_timeout(left).expect("timed out");
            at = t;
            self.diag = e.diagnostics;
            if let SyncUpdate::Reconciled(r) = e.update {
                self.state = *r;
            }
        }
        at
    }

    /// Drains updates until the worker has been quiet for 600 ms.
    fn settle(&mut self) {
        while let Ok((_, e)) = self.rx.recv_timeout(Duration::from_millis(600)) {
            self.diag = e.diagnostics;
            if let SyncUpdate::Reconciled(r) = e.update {
                self.state = *r;
            }
        }
    }
}

fn list_len(r: &Reconciled, c: DiffComparison) -> usize {
    let i = DiffComparison::ALL.iter().position(|&m| m == c).unwrap();
    r.lists[i].as_ref().map_or(0, |l| l.len())
}

struct Row {
    events: Vec<f64>,
    runs: Vec<f64>,
    git: Vec<f64>,
    reconcile_ms: Vec<f64>,
    latency_ms: Vec<f64>,
}

fn scenario(
    s: &mut Session,
    repo: &Path,
    runs: usize,
    act: &dyn Fn(&Path, usize),
    done: &dyn Fn(&Reconciled) -> bool,
    reset: &dyn Fn(&Path),
) -> Row {
    let mut row = Row {
        events: vec![],
        runs: vec![],
        git: vec![],
        reconcile_ms: vec![],
        latency_ms: vec![],
    };
    for run in 0..=runs {
        reset(repo);
        s.settle();
        let before = s.diag;
        act(repo, run);
        let finished = Instant::now();
        let at = s.until(|r, _| done(r), Duration::from_secs(60));
        s.settle();
        if run == 0 {
            continue; // warm-up
        }
        let d = s.diag;
        let n = (d.reconcile_requests - before.reconcile_requests) as f64;
        row.events.push((d.fs_events - before.fs_events) as f64);
        row.runs.push(n);
        row.git.push((d.git_calls - before.git_calls) as f64);
        row.reconcile_ms
            .push((d.total_reconcile - before.total_reconcile).as_secs_f64() * 1000.0 / n.max(1.0));
        row.latency_ms
            .push(at.saturating_duration_since(finished).as_secs_f64() * 1000.0);
    }
    row
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(5);
    let base = std::env::temp_dir().join("diffkit-bench-sync");
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    println!("Release build; real watcher; quiet 200 ms, maximum delay 1 s; {runs} measured runs after 1 warm-up (medians).\n");
    println!("Latency = end of the external change → published state (includes the 200 ms quiet period).\n");
    println!("| Tracked | Scenario | Events | Reconciliations | Git processes | Reconcile time | Latency |");
    println!("|---:|---|---:|---:|---:|---:|---:|");

    let clean = |repo: &Path| {
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["reset", "-q", "--hard"]);
        git(repo, &["clean", "-q", "-fd"]);
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["branch", "-q", "-D", "bench-branch"])
            .output();
    };
    for tracked in [100usize, 5_000, 50_000] {
        let repo = build(&base, tracked);
        clean(&repo);
        let mut s = Session::start(&repo);
        let print = |name: &str, r: Row| {
            println!(
                "| {tracked} | {name} | {:.0} | {:.0} | {:.0} | {:.1} ms | {:.0} ms |",
                median(r.events),
                median(r.runs),
                median(r.git),
                median(r.reconcile_ms),
                median(r.latency_ms)
            );
        };
        let edit = |repo: &Path, i: usize, tag: &str| {
            write(repo, &file_path(i), format!("{}// {tag}\n", content(i)))
        };
        let row = scenario(
            &mut s,
            &repo,
            runs,
            &|r, n| edit(r, 0, &format!("save {n}")),
            &|r| list_len(r, DiffComparison::Unstaged) == 1,
            &clean,
        );
        print("one file saved", row);
        let row = scenario(
            &mut s,
            &repo,
            runs,
            &|r, n| {
                for k in 0..10 {
                    edit(r, 1, &format!("save {n}.{k}"));
                }
            },
            &|r| list_len(r, DiffComparison::Unstaged) == 1,
            &clean,
        );
        print("10 events, one file", row);
        let row = scenario(
            &mut s,
            &repo,
            runs,
            &|r, n| {
                for k in 0..100 {
                    edit(r, k % 10, &format!("save {n}.{k}"));
                }
            },
            &|r| list_len(r, DiffComparison::Unstaged) == 10,
            &clean,
        );
        print("100 events, 10 files", row);
        let wide = tracked.min(100);
        let row = scenario(
            &mut s,
            &repo,
            runs,
            &|r, n| {
                for k in 0..1000 {
                    edit(r, k % wide, &format!("burst {n}.{k}"));
                }
            },
            &|r| list_len(r, DiffComparison::Unstaged) == wide,
            &clean,
        );
        print("1,000-event burst", row);
        let row = scenario(
            &mut s,
            &repo,
            runs.min(3),
            &|r, n| {
                let began = Instant::now();
                let mut k = 0;
                while began.elapsed() < Duration::from_secs(3) {
                    edit(r, 2, &format!("typing {n}.{k}"));
                    k += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
            },
            &|r| list_len(r, DiffComparison::Unstaged) == 1,
            &clean,
        );
        print("save every 50 ms for 3 s", row);
        let row = scenario(
            &mut s,
            &repo,
            runs,
            &|r, n| {
                edit(r, 3, &format!("stage {n}"));
                git(r, &["add", &file_path(3)]);
            },
            &|r| list_len(r, DiffComparison::Staged) == 1,
            &clean,
        );
        print("edit + git add (index)", row);
        let row = scenario(
            &mut s,
            &repo,
            runs,
            &|r, _| git(r, &["switch", "-q", "-c", "bench-branch"]),
            &|r| r.snapshot.branch.name() == Some("bench-branch"),
            &clean,
        );
        print("branch switch", row);
        // Manual refresh: request, then wait for the result.
        let mut manual = Vec::new();
        let mut manual_git = Vec::new();
        for _ in 0..runs {
            s.settle();
            let before = s.diag;
            let t = Instant::now();
            s.service.request_refresh();
            let at = s.until(
                |_, d| {
                    d.reconcile_requests > before.reconcile_requests
                        && d.reconciles_completed > before.reconciles_completed
                },
                Duration::from_secs(60),
            );
            manual.push(at.saturating_duration_since(t).as_secs_f64() * 1000.0);
            manual_git.push((s.diag.git_calls - before.git_calls) as f64);
        }
        println!(
            "| {tracked} | manual refresh (full reconciliation) | 0 | 1 | {:.0} | — | {:.0} ms |",
            median(manual_git),
            median(manual)
        );
        clean(&repo);
        assert!(s.service.shutdown(Duration::from_secs(5)));
    }

    println!("\n## Cache (5,000-file repository, 200 changed files, interface state in-process)\n");
    let repo = base.join("repo-5000");
    for i in 0..200 {
        write(
            &repo,
            &file_path(i),
            format!("{}// cache bench\n", content(i)),
        );
    }
    let size = Rect::new(0, 0, 200, 60);
    let mut app = App::open(&repo, size).unwrap();
    app.load_pending();
    let calls = thread_invocations();
    for _ in 0..50 {
        app.dispatch(Action::SelectNextFile);
        app.dispatch(Action::SelectPreviousFile);
    }
    let s = app.cache_stats();
    println!(
        "- Back and forth between two files, 100 moves: {} Git calls, {} hits / {} misses.",
        thread_invocations() - calls,
        s.hits,
        s.misses
    );
    let mut app = App::open(&repo, size).unwrap();
    app.load_pending();
    let walk = |app: &mut App| {
        app.dispatch(Action::SelectFirstFile);
        for _ in 0..199 {
            app.dispatch(Action::SelectNextFile);
        }
    };
    walk(&mut app);
    let first = app.cache_stats();
    let calls = thread_invocations();
    let t = Instant::now();
    walk(&mut app);
    let second = app.cache_stats();
    println!(
        "- 200 files, second pass (64 MiB budget): {} Git calls in {:.1} ms; hit ratio {:.0}%; {} entries, {:.2} MiB (peak {:.2} MiB).",
        thread_invocations() - calls,
        t.elapsed().as_secs_f64() * 1000.0,
        100.0 * (second.hits - first.hits) as f64 / ((second.hits + second.misses) - (first.hits + first.misses)) as f64,
        second.entries,
        second.bytes as f64 / 1048576.0,
        second.peak_bytes as f64 / 1048576.0
    );
    let mut app = App::open(&repo, size).unwrap();
    app.load_pending();
    app.set_cache_budget(256 << 10);
    walk(&mut app);
    let calls = thread_invocations();
    walk(&mut app);
    let st = app.cache_stats();
    println!(
        "- Same walk with a 256 KiB budget: second pass {} Git calls; {} evictions; peak {:.0} KiB; resident {:.0} KiB.",
        thread_invocations() - calls,
        st.evictions,
        st.peak_bytes as f64 / 1024.0,
        st.bytes as f64 / 1024.0
    );
    let mut app = App::open(&repo, size).unwrap();
    app.load_pending();
    write(
        &repo,
        &file_path(0),
        format!("{}// invalidated\n", content(0)),
    );
    let r = reconcile::reconcile(&repo, DiffOptions::default(), ChangeSet::WORKTREE, None).unwrap();
    app.apply_reconciled(r);
    let calls = thread_invocations();
    let t = Instant::now();
    app.load_pending();
    println!(
        "- Reload after invalidation of the shown file: {:.2} ms, {} Git call; invalidations {}.",
        t.elapsed().as_secs_f64() * 1000.0,
        thread_invocations() - calls,
        app.cache_stats().invalidations
    );
    git(&repo, &["checkout", "-q", "--", "."]);

    let big = base.join("big");
    if !big.join(".git").exists() {
        std::fs::create_dir_all(&big).unwrap();
        git(&big, &["init", "-q", "-b", "main"]);
        write(
            &big,
            "large.txt",
            (0..200_000)
                .map(|i| format!("row {i:06} of the large text file\n"))
                .collect::<String>(),
        );
        git(&big, &["add", "-A"]);
        git(&big, &["commit", "-q", "-m", "big"]);
    }
    write(
        &big,
        "large.txt",
        (0..200_000)
            .map(|i| {
                if i % 100 == 50 {
                    format!("row {i:06} EDITED\n")
                } else {
                    format!("row {i:06} of the large text file\n")
                }
            })
            .collect::<String>(),
    );
    let mut app = App::open(&big, size).unwrap();
    app.load_pending();
    let st = app.cache_stats();
    println!(
        "- Large diff entry (200,000-line file, 16,000 rows): {:.2} MiB charged to the cache.",
        st.bytes as f64 / 1048576.0
    );
    git(&big, &["checkout", "-q", "--", "."]);
}
