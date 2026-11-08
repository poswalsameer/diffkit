//! Shared fixtures for integration tests.
//!
//! Every test works in its own temporary directory, removed on drop. Git
//! runs with the developer's global and system configuration disabled, a
//! local test identity, and discovery capped at the temporary base so no
//! test can reach a real repository.

// Each integration test crate compiles this module separately and uses a
// different subset of the helpers.
#![allow(dead_code)]

use diffkit::{
    git::snapshot,
    types::repository::{FileStatus, RepositorySnapshot},
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Once,
    },
};

/// The path as Git reports it: symlinks resolved on Unix, plain absolute
/// on Windows (matching `repository::discover_from`).
pub fn real(p: &Path) -> PathBuf {
    if cfg!(windows) {
        std::path::absolute(p).unwrap()
    } else {
        p.canonicalize().unwrap()
    }
}

fn base() -> PathBuf {
    let base = std::env::temp_dir().join("diffkit-tests");
    std::fs::create_dir_all(&base).unwrap();
    real(&base)
}

pub fn isolate() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        std::env::set_var("GIT_CONFIG_GLOBAL", null);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_CEILING_DIRECTORIES", base());
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_NAMESPACE",
        ] {
            std::env::remove_var(var);
        }
    });
}

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        isolate();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = base().join(format!("{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path: real(&path) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.path.join(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Runs Git for fixture setup and returns the raw output.
pub fn git_output<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Output {
    isolate();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("failed to run git")
}

/// Runs Git for fixture setup and panics with stderr if it fails.
pub fn git<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> String {
    let out = git_output(dir, args);
    assert!(
        out.status.success(),
        "git {:?} failed in {}:\n{}",
        args.iter()
            .map(|a| a.as_ref().to_string_lossy())
            .collect::<Vec<_>>(),
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[derive(Clone)]
pub struct Repo {
    pub path: PathBuf,
}

impl Repo {
    pub fn init(path: impl Into<PathBuf>) -> Self {
        Self::init_with_branch(path, "main")
    }

    pub fn init_with_branch(path: impl Into<PathBuf>, branch: &str) -> Self {
        let path = path.into();
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", branch]);
        let repo = Self { path: real(&path) };
        repo.identity();
        repo
    }

    pub fn init_bare(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "--bare", "-b", "main"]);
        Self { path: real(&path) }
    }

    pub fn clone_from(source: &Path, dest: impl Into<PathBuf>) -> Self {
        let dest = dest.into();
        let parent = dest.parent().unwrap();
        git(
            parent,
            &[
                OsStr::new("clone"),
                OsStr::new("-q"),
                source.as_os_str(),
                dest.as_os_str(),
            ],
        );
        let repo = Self { path: real(&dest) };
        repo.identity();
        repo
    }

    fn identity(&self) {
        self.git(&["config", "user.name", "Diffkit Test"]);
        self.git(&["config", "user.email", "test@diffkit.invalid"]);
        self.git(&["config", "commit.gpgsign", "false"]);
    }

    pub fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> String {
        git(&self.path, args)
    }

    pub fn git_output<S: AsRef<OsStr>>(&self, args: &[S]) -> Output {
        git_output(&self.path, args)
    }

    pub fn write(&self, rel: impl AsRef<Path>, contents: &str) {
        let p = self.path.join(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    pub fn remove(&self, rel: impl AsRef<Path>) {
        std::fs::remove_file(self.path.join(rel)).unwrap();
    }

    /// Stages everything and commits; returns the new HEAD.
    pub fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.head()
    }

    pub fn empty_commit(&self, message: &str) -> String {
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.head()
    }

    pub fn rev(&self, rev: &str) -> String {
        self.git(&["rev-parse", "--verify", rev]).trim().to_owned()
    }

    pub fn head(&self) -> String {
        self.rev("HEAD")
    }

    /// `git rev-list --left-right --count HEAD...<upstream>`, the reference
    /// computation the snapshot's counts are checked against.
    pub fn rev_list_counts(&self, upstream: &str) -> (u64, u64) {
        let out = self.git(&[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{upstream}"),
        ]);
        let mut n = out.split_whitespace().map(|x| x.parse().unwrap());
        (n.next().unwrap(), n.next().unwrap())
    }

    pub fn snapshot(&self) -> RepositorySnapshot {
        inspect(&self.path)
    }
}

pub fn inspect(path: &Path) -> RepositorySnapshot {
    isolate();
    snapshot::inspect(path).unwrap_or_else(|e| panic!("inspect {} failed: {e}", path.display()))
}

/// A work repository whose `main` tracks `origin/main` in a local bare
/// remote, plus a second clone that can publish commits to that remote.
pub struct Remote {
    pub tmp: TempDir,
    pub work: Repo,
    pub bare: Repo,
    pub other: Repo,
}

impl Remote {
    pub fn new(label: &str) -> Self {
        let tmp = TempDir::new(label);
        let bare = Repo::init_bare(tmp.join("remote.git"));
        let work = Repo::init(tmp.join("work"));
        work.write("file.txt", "base\n");
        work.commit_all("base");
        work.git(&[
            OsStr::new("remote"),
            OsStr::new("add"),
            OsStr::new("origin"),
            bare.path.as_os_str(),
        ]);
        work.git(&["push", "-q", "-u", "origin", "main"]);
        let other = Repo::clone_from(&bare.path, tmp.join("other"));
        Self {
            tmp,
            work,
            bare,
            other,
        }
    }

    /// Publishes `n` new commits to the remote's `main` from the other clone.
    pub fn publish_remote_commits(&self, n: usize) {
        self.other.git(&["pull", "-q", "--ff-only"]);
        for i in 0..n {
            self.other.empty_commit(&format!("remote {i}"));
        }
        self.other.git(&["push", "-q", "origin", "main"]);
    }
}

pub fn find(s: &RepositorySnapshot, path: impl AsRef<Path>) -> &FileStatus {
    let path = path.as_ref();
    s.status
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{} not in status: {:#?}", path.display(), s.status.files))
}

pub fn xy(f: &FileStatus) -> String {
    String::from_utf8_lossy(&f.xy).into_owned()
}

/// Runs the built `diffkit` binary in `cwd`.
pub fn diffkit(cwd: &Path, args: &[&str]) -> Output {
    isolate();
    Command::new(env!("CARGO_BIN_EXE_diffkit"))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .expect("failed to run diffkit")
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

// ---------------------------------------------------------------- diff helpers

use diffkit::{
    diff::engine::DiffEngine,
    types::diff::{DiffComparison, DiffFile, DiffFileList, DiffOptions, DiffRowKind, FileDiff},
};

pub fn engine(path: &Path) -> DiffEngine {
    engine_with(path, DiffOptions::default())
}

pub fn engine_with(path: &Path, options: DiffOptions) -> DiffEngine {
    isolate();
    DiffEngine::open(path, options).unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
}

pub fn discover(engine: &DiffEngine, c: DiffComparison) -> DiffFileList {
    engine
        .discover(c)
        .unwrap_or_else(|e| panic!("discover {c:?}: {e}"))
}

/// The entry whose new path (or old path, for deletions) is `path`.
pub fn entry(list: &DiffFileList, path: impl AsRef<Path>) -> &DiffFile {
    let path = path.as_ref();
    list.files
        .iter()
        .find(|f| f.path() == path)
        .unwrap_or_else(|| {
            panic!(
                "{} not in {:?}: {:#?}",
                path.display(),
                list.comparison,
                list.files
            )
        })
}

/// Discovers `c` and loads the diff of `path`.
pub fn load(engine: &DiffEngine, c: DiffComparison, path: impl AsRef<Path>) -> FileDiff {
    let list = discover(engine, c);
    let id = entry(&list, path).id;
    engine
        .load_file_diff(&list, id)
        .unwrap_or_else(|e| panic!("load: {e}"))
}

/// (kind, old line number, new line number, old text, new text)
pub type RowSummary = (DiffRowKind, Option<usize>, Option<usize>, String, String);

pub fn rows(d: &FileDiff) -> Vec<RowSummary> {
    let text = |l: &Option<diffkit::types::diff::DiffLine>| {
        l.as_ref()
            .map(|l| String::from_utf8_lossy(&l.content).into_owned())
            .unwrap_or_default()
    };
    d.rows()
        .map(|r| {
            (
                r.kind,
                r.old_line.as_ref().map(|l| l.line_number),
                r.new_line.as_ref().map(|l| l.line_number),
                text(&r.old_line),
                text(&r.new_line),
            )
        })
        .collect()
}

pub fn row(
    kind: DiffRowKind,
    old: Option<usize>,
    new: Option<usize>,
    old_text: &str,
    new_text: &str,
) -> RowSummary {
    (kind, old, new, old_text.into(), new_text.into())
}

/// Numbered lines "prefix1\n…prefixN\n".
pub fn numbered(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix}{i}\n")).collect()
}

// ----------------------------------------------------------------- tui helpers

use diffkit::tui::{app::App, ui};
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};

/// Opens the TUI state for `path` at the given size and loads the selected
/// file, as the event loop does before its first frame.
pub fn tui_app(path: &Path, width: u16, height: u16) -> App {
    isolate();
    let mut app = App::open(path, Rect::new(0, 0, width, height)).expect("open app");
    app.load_pending();
    app
}

/// Renders the app into a test backend of the app's size.
pub fn render_buffer(app: &App) -> Buffer {
    let size = app.size();
    let mut t = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
    t.draw(|f| ui::render(f, app)).unwrap();
    t.backend().buffer().clone()
}

/// The rendered screen as one string per row.
pub fn screen(app: &App) -> Vec<String> {
    lines_of(&render_buffer(app))
}

pub fn lines_of(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (area.y..area.y + area.height)
        .map(|y| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect()
        })
        .collect()
}

pub fn screen_text(app: &App) -> String {
    screen(app).join("\n")
}

// ------------------------------------------------------------ read-only proof

/// One entry of a [`fingerprint`]: path, content (a symlink's target for
/// symlinks), and modification time.
pub type Print = (PathBuf, Vec<u8>, Option<std::time::SystemTime>);

/// Every file and symlink under `dir`, including `.git` (index, refs,
/// HEAD, objects, state files): content and modification time. Symlinks
/// are recorded, never followed. Two equal fingerprints mean nothing in
/// the repository or its working tree was written.
pub fn fingerprint(dir: &Path) -> Vec<Print> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.file_type().is_symlink() {
                let target = std::fs::read_link(&p).unwrap();
                out.push((p, target.to_string_lossy().into_owned().into_bytes(), None));
            } else if m.is_dir() {
                out.push((p.clone(), Vec::new(), None));
                stack.push(p);
            } else {
                let content = std::fs::read(&p).unwrap_or_default();
                out.push((p, content, m.modified().ok()));
            }
        }
    }
    out.sort();
    out
}

/// Asserts that `f` leaves `dir` byte-for-byte unchanged.
pub fn assert_read_only<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let before = fingerprint(dir);
    let result = f();
    let after = fingerprint(dir);
    if before != after {
        let changed: Vec<_> = after
            .iter()
            .filter(|a| !before.contains(a))
            .chain(before.iter().filter(|b| !after.contains(b)))
            .map(|(p, _, _)| p.display().to_string())
            .collect();
        panic!("inspection wrote to the repository: {changed:#?}");
    }
    result
}

impl Repo {
    /// Creates a symlink at `rel` pointing to `target` (Unix only).
    #[cfg(unix)]
    pub fn symlink(&self, target: &str, rel: &str) {
        let p = self.path.join(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        let _ = std::fs::remove_file(&p);
        std::os::unix::fs::symlink(target, p).unwrap();
    }

    pub fn write_bytes(&self, rel: impl AsRef<Path>, bytes: &[u8]) {
        let p = self.path.join(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(p, bytes).unwrap();
    }

    /// `git merge <branch>`, which must stop with conflicts.
    pub fn failing_merge(&self, branch: &str) {
        let out = self.git_output(&["merge", "--no-edit", branch]);
        assert!(
            !out.status.success(),
            "expected a conflict merging {branch}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
