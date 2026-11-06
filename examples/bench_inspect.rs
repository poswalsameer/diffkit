//! Reproducible `diffkit inspect` benchmark.
//!
//! cargo build --release && cargo run --release --example bench_inspect [-- RUNS]
//!
//! Builds deterministic repositories (100, 5,000, and 50,000 tracked files,
//! each tracking a local bare remote) under the system temp directory, then
//! times the release binary end to end: process start, all Git
//! subprocesses, parsing, and report output to /dev/null. Each scenario gets
//! warm-up runs followed by measured runs; individual timings and the
//! median are printed. A raw `git status` with the same arguments is timed
//! as a baseline. Peak memory comes from `/usr/bin/time -l` (macOS) or
//! `/usr/bin/time -v` (Linux); Git subprocess counts from a logging wrapper.
//!
//! Fixture setup runs Git commands that write (add, commit, push); those
//! are the benchmark's own, never Diffkit's.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const WARMUP: usize = 3;
const STATUS_ARGS: [&str; 6] = diffkit::git::status::STATUS_ARGS;

fn git<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> String {
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
        "git {:?}: {}",
        args.iter()
            .map(|a| a.as_ref().to_string_lossy())
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn file_path(i: usize) -> String {
    format!("src/mod{:03}/file{i:06}.rs", i / 100)
}

/// Creates (or reuses) a committed repository with `tracked` files that
/// tracks `origin/main` in a sibling bare repository.
fn build(base: &Path, tracked: usize) -> PathBuf {
    let repo = base.join(format!("repo-{tracked}"));
    let done = repo.join(".git/bench-ready");
    if done.exists() {
        return repo;
    }
    let _ = std::fs::remove_dir_all(&repo);
    let bare = base.join(format!("remote-{tracked}.git"));
    let _ = std::fs::remove_dir_all(&bare);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for i in 0..tracked {
        let p = repo.join(file_path(i));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            format!("// file {i}\npub fn f{i}() -> usize {{ {i} }}\n"),
        )
        .unwrap();
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    git(
        base,
        &[
            OsStr::new("init"),
            OsStr::new("-q"),
            OsStr::new("--bare"),
            bare.as_os_str(),
        ],
    );
    git(
        &repo,
        &[
            OsStr::new("remote"),
            OsStr::new("add"),
            OsStr::new("origin"),
            bare.as_os_str(),
        ],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "local, not pushed"],
    );
    // Let the index become older than every file, then refresh it, so no
    // entry is "racily clean" (which would force content re-reads on every
    // run because Diffkit never writes the refreshed index back).
    std::thread::sleep(Duration::from_millis(1100));
    git(&repo, &["update-index", "-q", "--really-refresh"]);
    std::fs::write(&done, "").unwrap();
    repo
}

/// Applies a deterministic set of changes and returns a restore closure.
fn dirty(repo: &Path, tracked: usize) -> impl FnOnce() + '_ {
    let modified = (tracked * 2 / 100).max(10);
    for i in 0..modified {
        let p = repo.join(file_path(i));
        std::fs::write(&p, format!("// changed {i}\n")).unwrap();
        if i >= modified / 2 {
            git(repo, &["add", &file_path(i)]);
            if i >= modified * 3 / 4 {
                std::fs::write(&p, format!("// changed again {i}\n")).unwrap();
            }
        }
    }
    for i in modified..modified + (modified / 10).max(1) {
        std::fs::remove_file(repo.join(file_path(i))).unwrap();
    }
    for i in 0..modified / 2 {
        std::fs::write(
            repo.join(format!("untracked/new{i:05}.txt")).tap_dir(),
            "new",
        )
        .unwrap();
    }
    move || {
        git(repo, &["reset", "-q", "--hard"]);
        git(repo, &["clean", "-q", "-fd"]);
        std::thread::sleep(Duration::from_millis(1100));
        git(repo, &["update-index", "-q", "--really-refresh"]);
    }
}

trait TapDir {
    fn tap_dir(self) -> Self;
}
impl TapDir for PathBuf {
    fn tap_dir(self) -> Self {
        std::fs::create_dir_all(self.parent().unwrap()).unwrap();
        self
    }
}

fn time_runs(runs: usize, mut f: impl FnMut()) -> Vec<f64> {
    for _ in 0..WARMUP {
        f();
    }
    (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect()
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

fn run_diffkit(bin: &Path, repo: &Path) {
    let st = Command::new(bin)
        .arg("inspect")
        .current_dir(repo)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());
}

fn run_git_status(repo: &Path) {
    let st = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(STATUS_ARGS)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());
}

/// Memory reported by /usr/bin/time, in KiB.
struct Memory {
    /// "maximum resident set size": the maximum over the measured process
    /// and the children it waited for, so for diffkit this is usually a Git
    /// subprocess.
    tree_max_rss: Option<u64>,
    /// "peak memory footprint" (macOS only): the measured process alone.
    own_footprint: Option<u64>,
}

fn memory(cmd: &[&OsStr], cwd: &Path) -> Memory {
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let mut m = Memory {
        tree_max_rss: None,
        own_footprint: None,
    };
    let Ok(out) = Command::new("/usr/bin/time")
        .arg(flag)
        .args(cmd)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::null())
        .output()
    else {
        return m;
    };
    let err = String::from_utf8_lossy(&out.stderr);
    let bytes = |v: &str| v.trim().parse::<u64>().ok().map(|b| b / 1024);
    for line in err.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_suffix("maximum resident set size") {
            m.tree_max_rss = bytes(v); // bytes on macOS
        } else if let Some(v) = l.strip_suffix("peak memory footprint") {
            m.own_footprint = bytes(v);
        } else if let Some(v) = l.strip_prefix("Maximum resident set size (kbytes):") {
            m.tree_max_rss = v.trim().parse().ok();
        }
    }
    m
}

#[cfg(unix)]
fn count_git_calls(bin: &Path, repo: &Path, scratch: &Path) -> usize {
    use std::os::unix::fs::PermissionsExt;
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH");
    let real = real.display();
    let dir = scratch.join("wrapper");
    std::fs::create_dir_all(&dir).unwrap();
    let log = scratch.join("calls.log");
    let _ = std::fs::remove_file(&log);
    let w = dir.join("git");
    std::fs::write(
        &w,
        format!(
            "#!/bin/sh\necho x >> '{}'\nexec '{real}' \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o755)).unwrap();
    let st = Command::new(bin)
        .arg("inspect")
        .current_dir(repo)
        .env("PATH", &dir)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());
    std::fs::read_to_string(&log).unwrap().lines().count()
}

fn sh(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unavailable".into())
}

/// Times the first and second launch of freshly copied binaries running
/// `--version` (no Git involved), isolating per-binary launch overhead.
fn first_launch(bin: &Path, scratch: &Path) {
    println!("\n### First launch of a newly written binary (`diffkit --version`, no Git)\n");
    println!("| Trial | First launch (ms) | Second launch (ms) |\n|---:|---:|---:|");
    for trial in 0..3 {
        let copy = scratch.join(format!("diffkit-copy-{}-{trial}", std::process::id()));
        std::fs::copy(bin, &copy).unwrap();
        let mut t = Vec::new();
        for _ in 0..2 {
            let s = Instant::now();
            assert!(Command::new(&copy)
                .arg("--version")
                .stdout(Stdio::null())
                .status()
                .unwrap()
                .success());
            t.push(s.elapsed().as_secs_f64() * 1000.0);
        }
        println!("| {trial} | {:.2} | {:.2} |", t[0], t[1]);
        let _ = std::fs::remove_file(&copy);
    }
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bin = root.join("target/release/diffkit");
    assert!(bin.exists(), "run `cargo build --release` first");
    let base = std::env::temp_dir().join("diffkit-bench");
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
    println!(
        "- Memory: {} GiB",
        sh("sysctl", &["-n", "hw.memsize"])
            .parse::<u64>()
            .map_or("?".into(), |b| (b >> 30).to_string())
    );
    println!("- {}", sh("git", &["--version"]));
    println!("- Warm-up runs: {WARMUP}; measured runs: {runs}\n");

    println!("## Results\n");
    println!("| Repo | Tracked | Scenario | Changed entries | Git calls | diffkit median (ms) | git status median (ms) | diffkit own peak footprint (MiB) | Max RSS in diffkit process tree (MiB) | git status max RSS (MiB) |");
    println!("|---|---:|---|---:|---:|---:|---:|---:|---:|---:|");
    let mut detail = Vec::new();
    for (label, tracked) in [("small", 100), ("medium", 5_000), ("large", 50_000)] {
        let repo = build(&base, tracked);
        for scenario in ["clean", "dirty"] {
            let restore = (scenario == "dirty").then(|| dirty(&repo, tracked));
            let snap = diffkit::git::snapshot::inspect(&repo).unwrap();
            let st = &snap.status;
            let changed = st.files.len();
            let calls = {
                #[cfg(unix)]
                {
                    count_git_calls(&bin, &repo, &base).to_string()
                }
                #[cfg(not(unix))]
                {
                    "n/a".to_string()
                }
            };
            let d = time_runs(runs, || run_diffkit(&bin, &repo));
            let g = time_runs(runs, || run_git_status(&repo));
            let mem_d = memory(&[bin.as_os_str(), OsStr::new("inspect")], &repo);
            let mut gcmd: Vec<&OsStr> = vec![OsStr::new("git")];
            gcmd.extend(STATUS_ARGS.iter().map(OsStr::new));
            let mem_g = memory(&gcmd, &repo);
            let mib =
                |k: Option<u64>| k.map_or("n/a".into(), |k| format!("{:.1}", k as f64 / 1024.0));
            println!(
                "| {label} | {tracked} | {scenario} | {changed} | {calls} | {:.2} | {:.2} | {} | {} | {} |",
                median(&d),
                median(&g),
                mib(mem_d.own_footprint),
                mib(mem_d.tree_max_rss),
                mib(mem_g.tree_max_rss)
            );
            detail.push(format!(
                "- {label}/{scenario} (staged {}, unstaged {}, partially staged {}, untracked {}, deleted {}; ahead {:?}):\n  - diffkit: {}\n  - git status: {}",
                st.staged_count(),
                st.unstaged_count(),
                st.partially_staged_count(),
                st.untracked_count(),
                st.deleted_count(),
                snap.commit_tracking.ahead(),
                d.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(", "),
                g.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(", "),
            ));
            if let Some(restore) = restore {
                restore();
            }
        }
    }
    println!("\n### Individual measured runs (ms)\n");
    for d in detail {
        println!("{d}");
    }
    first_launch(&bin, &base);
}
