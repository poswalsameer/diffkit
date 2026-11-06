//! Reproducible diff-engine benchmark.
//!
//! cargo build --release && cargo run --release --example bench_diff [-- RUNS]
//!
//! Builds deterministic repositories (100, 5,000, 50,000 tracked files) and a
//! special-case repository under the system temp directory, applies
//! deterministic changes, and times the library in-process: discovery,
//! loading one file, and `load_all`, each with warm-up runs and the median
//! of the measured runs. Git subprocesses are counted with
//! `git::commands::thread_invocations`. For single large files, time is split
//! into the Git call, parsing, and hunk building (line numbering + Myers
//! alignment). Peak memory comes from `/usr/bin/time -l` running
//! `diffkit diff-stats --load` (macOS "peak memory footprint" is the diffkit
//! process alone).
//!
//! Fixture setup runs Git commands that write (add, commit, reset); those
//! are the benchmark's own, never the engine's.

use diffkit::{
    diff::{self, DiffComparison, DiffEngine, DiffOptions},
    git::{self, commands::thread_invocations},
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const WARMUP: usize = 2;

fn git<S: AsRef<OsStr>>(dir: &Path, args: &[S]) {
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

fn file_path(i: usize) -> String {
    format!("src/mod{:03}/file{i:06}.rs", i / 100)
}

fn content(i: usize) -> String {
    (0..20)
        .map(|l| format!("// file {i} line {l}\npub fn f{i}_{l}() -> usize {{ {l} }}\n"))
        .collect()
}

fn settle(repo: &Path) {
    std::thread::sleep(Duration::from_millis(1100));
    git(repo, &["update-index", "-q", "--really-refresh"]);
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
    settle(&repo);
    std::fs::write(repo.join(".git/bench-ready"), "").unwrap();
    repo
}

fn write(repo: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn restore(repo: &Path) {
    git(repo, &["reset", "-q", "--hard"]);
    git(repo, &["clean", "-q", "-fd"]);
    settle(repo);
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Median milliseconds and the Git subprocesses of one run.
fn measure(runs: usize, mut f: impl FnMut()) -> (f64, Vec<f64>, u64) {
    for _ in 0..WARMUP {
        f();
    }
    let before = thread_invocations();
    f();
    let calls = thread_invocations() - before;
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let s = Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    let all = t.clone();
    (median(&mut t), all, calls)
}

fn fmt_runs(v: &[f64]) -> String {
    v.iter()
        .map(|x| format!("{x:.2}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// diffkit's own peak footprint and the process-tree max RSS, in MiB.
fn memory(bin: &Path, repo: &Path) -> (String, String) {
    let Ok(out) = Command::new("/usr/bin/time")
        .arg("-l")
        .arg(bin)
        .args(["diff-stats", "--load"])
        .current_dir(repo)
        .stdout(Stdio::null())
        .output()
    else {
        return ("n/a".into(), "n/a".into());
    };
    let err = String::from_utf8_lossy(&out.stderr);
    let find = |suffix: &str| {
        err.lines()
            .find_map(|l| l.trim().strip_suffix(suffix))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map_or("n/a".into(), |b| format!("{:.1}", b / 1048576.0))
    };
    (
        find("peak memory footprint"),
        find("maximum resident set size"),
    )
}

fn sh(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unavailable".into())
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(7);
    let bin_exe = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/diffkit");
    assert!(bin_exe.exists(), "run `cargo build --release` first");
    let base = std::env::temp_dir().join("diffkit-bench-diff");
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();

    println!("## Environment\n");
    println!(
        "- OS: {} {} ({})",
        sh("sw_vers", &["-productName"]),
        sh("sw_vers", &["-productVersion"]),
        sh("uname", &["-m"])
    );
    println!(
        "- CPU: {} ({} logical cores)",
        sh("sysctl", &["-n", "machdep.cpu.brand_string"]),
        sh("sysctl", &["-n", "hw.ncpu"])
    );
    println!("- {}", sh("git", &["--version"]));
    println!("- Build: release (in-process library timings)");
    println!("- Warm-up runs: {WARMUP}; measured runs: {runs}\n");

    println!("## Repository scenarios\n");
    println!("| Repo | Tracked | Scenario | Changed (unstaged / staged) | Discover unstaged | Load 1 file | load_all unstaged | Git calls (discover / load 1 / load_all) | diffkit footprint / tree max RSS (MiB) |");
    println!("|---|---:|---|---:|---:|---:|---:|---|---:|");
    let mut details = Vec::new();
    for (label, tracked) in [("small", 100usize), ("medium", 5_000), ("large", 50_000)] {
        let repo = build(&base, &format!("repo-{tracked}"), |r| {
            for i in 0..tracked {
                write(r, &file_path(i), content(i));
            }
        });
        for scenario in ["one file", "2% of files"] {
            let n = if scenario == "one file" {
                1
            } else {
                (tracked / 50).max(2)
            };
            for i in 0..n {
                let body = content(i)
                    .replace("line 3\n", "line three changed\n")
                    .replace("{ 7 }", "{ 70 }");
                write(&repo, &file_path(i), body);
                if i % 2 == 1 {
                    git(&repo, &["add", &file_path(i)]);
                }
            }
            let engine = DiffEngine::open(&repo, DiffOptions::default()).unwrap();
            let unstaged = engine.discover(DiffComparison::Unstaged).unwrap();
            let staged = engine.discover(DiffComparison::Staged).unwrap();
            let (d, d_all, d_calls) = measure(runs, || {
                engine.discover(DiffComparison::Unstaged).unwrap();
            });
            let first = unstaged.files[0].id;
            let (l, l_all, l_calls) = measure(runs, || {
                engine.load_file_diff(&unstaged, first).unwrap();
            });
            let (a, a_all, a_calls) = measure(runs, || {
                let c = engine.load_all(&unstaged).unwrap();
                assert!(c.files.iter().all(|f| f.result.is_ok()));
            });
            let (own, tree) = memory(&bin_exe, &repo);
            println!(
                "| {label} | {tracked} | {scenario} | {} / {} | {d:.2} ms | {l:.2} ms | {a:.2} ms | {d_calls} / {l_calls} / {a_calls} | {own} / {tree} |",
                unstaged.len(),
                staged.len()
            );
            details.push(format!(
                "- {label}/{scenario}: discover [{}]; load 1 [{}]; load_all [{}]",
                fmt_runs(&d_all),
                fmt_runs(&l_all),
                fmt_runs(&a_all)
            ));
            restore(&repo);
        }
    }

    println!("\n## Single-file scenarios (repository with 100 other files)\n");
    let special = build(&base, "special", |r| {
        for i in 0..100 {
            write(r, &file_path(i), content(i));
        }
        let big: String = (0..200_000)
            .map(|i| format!("row {i:06} of the large text file\n"))
            .collect();
        write(r, "large.txt", big);
        let bin: Vec<u8> = (0..20 * 1024 * 1024)
            .map(|i: u32| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        write(r, "blob.bin", bin);
        let block: String = (0..20_000)
            .map(|i| format!("original line {i}\n"))
            .collect();
        write(r, "rewrite.txt", &block);
        write(r, "partial.txt", &block);
    });
    let mut large = String::new();
    for i in 0..200_000 {
        if i % 100 == 50 {
            large.push_str(&format!("row {i:06} EDITED\n"));
        } else {
            large.push_str(&format!("row {i:06} of the large text file\n"));
        }
    }
    write(&special, "large.txt", large);
    let mut bin = std::fs::read(special.join("blob.bin")).unwrap();
    for b in bin.iter_mut().step_by(4096) {
        *b ^= 0xff;
    }
    write(&special, "blob.bin", bin);
    write(
        &special,
        "rewrite.txt",
        (0..20_000)
            .map(|i| format!("completely new content {i} xyz\n"))
            .collect::<String>(),
    );
    write(
        &special,
        "partial.txt",
        (0..20_000)
            .map(|i| {
                if i % 3 == 0 {
                    format!("changed line {i}\n")
                } else {
                    format!("original line {i}\n")
                }
            })
            .collect::<String>(),
    );
    let engine = DiffEngine::open(&special, DiffOptions::default()).unwrap();
    let list = engine.discover(DiffComparison::Unstaged).unwrap();
    println!("| File | Change | Git line counts | Load (median) | Git call | Parse | Hunks + alignment | Rows | Alignment |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---|");
    for (name, change) in [
        ("large.txt", "200,000 lines, every 100th edited"),
        ("blob.bin", "20 MiB binary, 5,120 bytes flipped"),
        ("rewrite.txt", "20,000 lines replaced, no common lines"),
        ("partial.txt", "20,000 lines, every 3rd changed"),
    ] {
        let f = list
            .files
            .iter()
            .find(|f| f.path() == Path::new(name))
            .unwrap();
        let (load, load_all, _) = measure(runs, || {
            engine.load_file_diff(&list, f.id).unwrap();
        });
        let loaded = engine.load_file_diff(&list, f.id).unwrap();
        let args = diff::commands::patch(
            DiffComparison::Unstaged,
            &engine.base(),
            3,
            &[Path::new(name)],
        );
        let (git_ms, _, _) = measure(runs, || {
            git::commands::output(&special, &args).unwrap();
        });
        let patch = git::commands::output(&special, &args).unwrap().stdout;
        let (parse_ms, _, _) = measure(runs, || {
            diff::parser::parse_patch(&patch).unwrap();
        });
        let parsed = diff::parser::parse_patch(&patch).unwrap();
        let (build_ms, _, _) = measure(runs, || {
            diff::hunk::build_hunks(&parsed[0].hunks, &DiffOptions::default().alignment).unwrap();
        });
        let quality = loaded
            .hunks()
            .iter()
            .any(|h| h.alignment == diff::AlignmentQuality::Bounded);
        println!(
            "| {name} | {change} | {} | {load:.2} ms | {git_ms:.2} ms | {parse_ms:.2} ms | {build_ms:.2} ms | {} | {} |",
            f.stats.map_or("binary".into(), |s| format!("+{} -{}", s.added, s.removed)),
            loaded.rows().count(),
            if quality { "bounded" } else { "exact" }
        );
        details.push(format!("- {name}: load [{}]", fmt_runs(&load_all)));
    }
    let (own, tree) = memory(&bin_exe, &special);
    println!("\n`diffkit diff-stats --load` on this repository: diffkit footprint {own} MiB, tree max RSS {tree} MiB.");
    restore(&special);

    println!("\n## Individual measured runs (ms)\n");
    for d in details {
        println!("{d}");
    }
}
