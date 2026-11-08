//! Phase 3 TUI benchmark (no terminal needed).
//!
//! cargo run --release --example bench_tui [-- RUNS]
//!
//! Measures, in-process with the release build:
//! - startup: `App::open` (snapshot + discovery of all four comparisons)
//!   and the first `load_pending`, with Git subprocess counts;
//! - the cost of turning a loaded diff into display rows, compared with the
//!   Phase 2 `load_file_diff` alone;
//! - one frame rendered into Ratatui's `TestBackend` (200x60);
//! - a scroll action followed by a frame;
//! - returning to an already loaded file.
//!
//! Repositories are generated deterministically under the temp directory.
//! Fixture setup runs writing Git commands; Diffkit does not.

use diffkit::{
    diff::engine::DiffEngine,
    git::commands::thread_invocations,
    tui::{actions::Action, app::App, diff_view::DiffView, ui},
    types::diff::{DiffComparison, DiffOptions},
};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use std::{
    path::{Path, PathBuf},
    process::Command,
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
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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

fn build(base: &Path, name: &str, setup: impl FnOnce(&Path)) -> PathBuf {
    let repo = base.join(name);
    if repo.join(".git/bench-ready").exists() {
        return repo;
    }
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    setup(&repo);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    std::thread::sleep(Duration::from_millis(1100));
    git(&repo, &["update-index", "-q", "--really-refresh"]);
    std::fs::write(repo.join(".git/bench-ready"), "").unwrap();
    repo
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

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let s = Instant::now();
    let t = f();
    (t, s.elapsed().as_secs_f64() * 1000.0)
}

fn render(term: &mut Terminal<TestBackend>, app: &App) {
    term.draw(|f| ui::render(f, app)).unwrap();
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(7);
    let base = std::env::temp_dir().join("diffkit-bench-tui");
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let size = Rect::new(0, 0, 200, 60);
    println!("Release build, in-process; {runs} measured runs after 1 warm-up; frames rendered into a 200x60 TestBackend.\n");

    println!("## Startup\n");
    println!("| Tracked | Changed (unstaged) | App::open | Git calls | First load | Git calls | Total to first diff |");
    println!("|---:|---:|---:|---:|---:|---:|---:|");
    for tracked in [100usize, 5_000, 50_000] {
        let repo = build(&base, &format!("repo-{tracked}"), |r| {
            for i in 0..tracked {
                write(r, &file_path(i), content(i));
            }
        });
        for i in 0..(tracked / 50).max(2) {
            write(
                &repo,
                &file_path(i),
                content(i).replace("line 3\n", "line three\n"),
            );
        }
        let mut opens = Vec::new();
        let mut loads = Vec::new();
        let (mut open_calls, mut load_calls) = (0, 0);
        for run in 0..=runs {
            let before = thread_invocations();
            let (mut app, t_open) = time(|| App::open(&repo, size).unwrap());
            open_calls = thread_invocations() - before;
            let before = thread_invocations();
            let ((), t_load) = time(|| app.load_pending());
            load_calls = thread_invocations() - before;
            if run > 0 {
                opens.push(t_open);
                loads.push(t_load);
            }
        }
        let changed = App::open(&repo, size)
            .unwrap()
            .mode_state(DiffComparison::Unstaged)
            .files()
            .len();
        let (o, l) = (median(opens), median(loads));
        println!(
            "| {tracked} | {changed} | {o:.2} ms | {open_calls} | {l:.2} ms | {load_calls} | {:.2} ms |",
            o + l
        );
        git(&repo, &["checkout", "-q", "--", "."]);
    }

    println!("\n## Large diff: 200,000-line file, every 100th line edited\n");
    let big = build(&base, "big", |r| {
        let body: String = (0..200_000)
            .map(|i| format!("row {i:06} of the large text file\n"))
            .collect();
        write(r, "large.txt", body);
    });
    let edited: String = (0..200_000)
        .map(|i| {
            if i % 100 == 50 {
                format!("row {i:06} EDITED\n")
            } else {
                format!("row {i:06} of the large text file\n")
            }
        })
        .collect();
    write(&big, "large.txt", edited);
    let engine = DiffEngine::open(&big, DiffOptions::default()).unwrap();
    let list = engine.discover(DiffComparison::Unstaged).unwrap();
    let id = list.files[0].id;
    let mut engine_only = Vec::new();
    let mut view_build = Vec::new();
    for run in 0..=runs {
        let (diff, t_load) = time(|| engine.load_file_diff(&list, id).unwrap());
        let (_view, t_view) = time(|| DiffView::new(diff));
        if run > 0 {
            engine_only.push(t_load);
            view_build.push(t_view);
        }
    }
    let mut app = App::open(&big, size).unwrap();
    let (_, t_first) = time(|| app.load_pending());
    let rows = app.view().unwrap().row_count();
    let mut term = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
    render(&mut term, &app);
    let mut frames = Vec::new();
    for _ in 0..runs.max(30) {
        let (_, t) = time(|| render(&mut term, &app));
        frames.push(t);
    }
    let mut scrolls = Vec::new();
    let before = thread_invocations();
    for i in 0..runs.max(30) {
        let action = if i % 2 == 0 {
            Action::PageDown
        } else {
            Action::NextHunk
        };
        let (_, t) = time(|| {
            app.dispatch(action);
            render(&mut term, &app);
        });
        scrolls.push(t);
    }
    let scroll_calls = thread_invocations() - before;
    app.dispatch(Action::ScrollBottom);
    let (_, t_bottom) = time(|| render(&mut term, &app));
    println!("| Measure | Median |");
    println!("|---|---:|");
    println!(
        "| Phase 2 `load_file_diff` alone | {:.2} ms |",
        median(engine_only)
    );
    println!(
        "| Building display rows (`DiffView::new`) | {:.2} ms |",
        median(view_build)
    );
    println!(
        "| TUI first load (`load_pending`, both of the above) | {t_first:.2} ms (single run) |"
    );
    println!("| Display rows | {rows} |");
    println!("| One frame at the top | {:.3} ms |", median(frames));
    println!(
        "| PageDown or ] then one frame | {:.3} ms ({scroll_calls} Git calls over all scrolls) |",
        median(scrolls)
    );
    println!("| One frame at the bottom | {t_bottom:.3} ms (single run) |");

    println!("\n## Returning to a loaded file\n");
    let repo = base.join("repo-5000");
    for i in 0..3 {
        write(
            &repo,
            &file_path(i),
            content(i).replace("line 3\n", "line three\n"),
        );
    }
    let mut app = App::open(&repo, size).unwrap();
    app.load_pending();
    app.dispatch(Action::SelectNextFile);
    let before = thread_invocations();
    let (_, t_back) = time(|| app.dispatch(Action::SelectPreviousFile));
    println!(
        "Back to the previous file: {t_back:.3} ms, {} Git calls (served from the cache).",
        thread_invocations() - before
    );
    git(&repo, &["checkout", "-q", "--", "."]);
}
