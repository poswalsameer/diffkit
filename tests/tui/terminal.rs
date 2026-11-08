//! The `diffkit` binary's interactive entry point: argument routing, and a
//! real pseudo-terminal session driven through `script(1)`.

use crate::support;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use support::numbered;
use support::{diffkit, stderr, stdout, Repo, TempDir};

#[test]
fn routing_without_a_terminal() {
    let tmp = TempDir::new("tui-cli-routing");
    let repo = Repo::init(tmp.join("repo"));
    for args in [&[][..], &[repo.path.to_str().unwrap()]] {
        let out = diffkit(tmp.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&out).contains("needs a terminal"),
            "{}",
            stderr(&out)
        );
        assert!(stdout(&out).is_empty());
    }
    let out = diffkit(tmp.path(), &["no-such-dir"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("not a command or a directory: no-such-dir"));
    let out = diffkit(tmp.path(), &[repo.path.to_str().unwrap(), "extra"]);
    assert_eq!(out.status.code(), Some(2));
    // Diagnostic commands are unchanged and non-interactive.
    assert_eq!(diffkit(&repo.path, &["inspect"]).status.code(), Some(0));
    assert_eq!(diffkit(&repo.path, &["diff-stats"]).status.code(), Some(0));
}

/// One step of a scripted terminal session.
#[cfg(any(target_os = "macos", target_os = "linux"))]
enum Step {
    /// Type these bytes, then pause 250 ms.
    Key(&'static [u8]),
    /// Do something outside Diffkit (edit a file, run Git).
    Run(Box<dyn FnOnce()>),
    Wait(u64),
    /// Send this signal (a `kill -s` name) to the Diffkit process.
    Signal(&'static str),
}

/// Runs `diffkit args…` inside a pseudo-terminal of 110x30, sends `keys`
/// (each after a pause), and returns (exit status, transcript bytes).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn pty_session(cwd: &Path, args: &[&str], keys: &[&'static [u8]]) -> Option<(i32, Vec<u8>)> {
    let steps = keys.iter().map(|k| Step::Key(k)).collect();
    pty_steps(cwd, args, steps, &[]).map(|(code, out, _)| (code, out))
}

/// Like [`pty_session`] with arbitrary steps and extra environment.
/// Also returns how long Diffkit took to exit after the last step.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn pty_steps(
    cwd: &Path,
    args: &[&str],
    steps: Vec<Step>,
    env: &[(&str, &std::ffi::OsStr)],
) -> Option<(i32, Vec<u8>, Duration)> {
    if Command::new("script").arg("-V").output().is_err() && !Path::new("/usr/bin/script").exists()
    {
        eprintln!("SKIPPED: script(1) is not available");
        return None;
    }
    support::isolate();
    let tmp = TempDir::new("pty");
    let transcript = tmp.join("transcript");
    let pidfile = tmp.join("pid");
    let bin = env!("CARGO_BIN_EXE_diffkit");
    let quoted: Vec<String> = args.iter().map(|a| format!("'{a}'")).collect();
    // `exec` keeps the shell's PID, so the recorded PID is Diffkit's.
    let inner = format!(
        "stty rows 30 cols 110; echo $$ > '{}'; exec '{bin}' {}",
        pidfile.display(),
        quoted.join(" ")
    );
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.arg("-q").arg(&transcript).args(["sh", "-c", &inner]);
    } else {
        cmd.args(["-q", "-e", "-c", &inner]).arg(&transcript);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take().unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    for step in steps {
        match step {
            Step::Key(k) => {
                stdin.write_all(k).unwrap();
                stdin.flush().unwrap();
                std::thread::sleep(Duration::from_millis(250));
            }
            Step::Run(f) => f(),
            Step::Wait(ms) => std::thread::sleep(Duration::from_millis(ms)),
            Step::Signal(sig) => {
                let pid = std::fs::read_to_string(&pidfile).unwrap();
                let ok = Command::new("kill")
                    .args(["-s", sig, pid.trim()])
                    .status()
                    .unwrap()
                    .success();
                assert!(ok, "kill -s {sig} {pid}");
            }
        }
    }
    let after_last_step = Instant::now();
    let deadline = after_last_step + Duration::from_secs(15);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("diffkit did not exit after q");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let exit_time = after_last_step.elapsed();
    drop(stdin);
    Some((
        status.code().unwrap_or(-1),
        std::fs::read(&transcript).unwrap_or_default(),
        exit_time,
    ))
}

/// Whether `needle` was drawn. Differential redraws skip cells whose
/// content did not change, and [`visible_text`] shows each skip as a
/// space, so a needle drawn over other text can have gaps ("FILES IN" over
/// "HISTORY" keeps its "I"). A gap may stand for any character, but at
/// least three quarters of the needle's non-space characters must match.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn drawn(text: &str, needle: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    let required = n.iter().filter(|c| **c != ' ').count() * 3 / 4;
    t.windows(n.len()).any(|w| {
        let mut exact = 0;
        for (a, b) in w.iter().zip(&n) {
            if a == b {
                exact += usize::from(*b != ' ');
            } else if *a != ' ' {
                return false;
            }
        }
        exact >= required.max(1)
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn visible_text(bytes: &[u8]) -> String {
    // Drop CSI sequences; what remains is the drawn text.
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i += 1;
            out.push(b' ');
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn interactive_session_in_a_real_pty() {
    let tmp = TempDir::new("tui-pty");
    let repo = Repo::init(tmp.join("demo"));
    repo.write(
        "app.js",
        "const name = \"Alice\";\nconst age = 20;\nconsole.log(name);\n",
    );
    repo.write("b.txt", &numbered("b ", 50));
    repo.commit_all("base");
    repo.write(
        "app.js",
        "const name = \"Alice\";\nconst age = 21;\nconsole.log(name);\n",
    );
    repo.write("b.txt", &numbered("b ", 50).replace("b 40\n", "b forty\n"));
    repo.write("new.txt", "untracked\n");
    let Some((code, out)) = pty_session(
        &repo.path,
        &[],
        &[b"j", b"]", b"G", b"?", b"?", b"1", b"4", b"\t", b"r", b"q"],
    ) else {
        return;
    };
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
    assert!(
        out.starts_with(b"\x1b[?1049h"),
        "enters the alternate screen first"
    );
    assert!(
        out.ends_with(b"\x1b[?1049l\x1b[?25h"),
        "leaves it and shows the cursor last"
    );
    assert!(
        !out.windows(4).any(|w| w == b"\x1b[6n"),
        "never asks for the cursor position"
    );
    let text = visible_text(&out);
    for needle in [
        "DIFFKIT",
        "UNSTAGED",
        "app.js",
        "const age = 20;",
        "const age = 21;",
        "Help",
        "UNTRACKED",
        "new.txt",
        "Refreshed",
    ] {
        assert!(text.contains(needle), "missing {needle:?}");
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn interactive_session_with_explicit_path() {
    let tmp = TempDir::new("tui-pty-path");
    let repo = Repo::init(tmp.join("elsewhere"));
    repo.write("x.txt", "x\n");
    let Some((code, out)) = pty_session(tmp.path(), &[repo.path.to_str().unwrap()], &[b"q"]) else {
        return;
    };
    assert_eq!(code, 0);
    let text = visible_text(&out);
    assert!(
        text.contains("elsewhere") && text.contains("x.txt"),
        "{text}"
    );
    assert!(out.ends_with(b"\x1b[?1049l\x1b[?25h"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn not_a_repository_fails_before_touching_the_terminal() {
    let tmp = TempDir::new("tui-pty-norepo");
    let plain = tmp.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let Some((code, out)) = pty_session(&plain, &[], &[]) else {
        return;
    };
    assert_eq!(code, 1);
    assert!(
        !out.windows(8).any(|w| w == b"\x1b[?1049h"),
        "no alternate screen for startup errors"
    );
    assert!(String::from_utf8_lossy(&out).contains("is not inside a Git repository"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn external_edits_appear_without_pressing_r() {
    let tmp = TempDir::new("tui-pty-sync");
    let repo = Repo::init(tmp.join("demo"));
    repo.write("app.js", "const age = 20;\n");
    repo.commit_all("base");
    repo.write("app.js", "const age = 21;\n");
    let path = repo.path.clone();
    let path2 = repo.path.clone();
    let steps = vec![
        Step::Wait(800),
        Step::Run(Box::new(move || {
            std::fs::write(
                path.join("app.js"),
                "const age = 42; // saved in an editor\n",
            )
            .unwrap()
        })),
        Step::Wait(1500),
        Step::Run(Box::new(move || {
            std::fs::write(path2.join("fresh.txt"), "new file\n").unwrap()
        })),
        Step::Wait(1500),
        Step::Key(b"4"),
        Step::Key(b"q"),
    ];
    let Some((code, out, exit_time)) = pty_steps(&repo.path, &[], steps, &[]) else {
        return;
    };
    assert_eq!(code, 0);
    let text = visible_text(&out);
    // Only changed cells are redrawn and the pane clips at its width, so
    // check the contiguous fragment of the new line.
    assert!(
        text.contains("saved in an"),
        "edit synchronized without r:\n{text}"
    );
    assert!(text.contains("fresh.txt"), "new untracked file appeared");
    assert!(text.contains("Synced"), "indicator shown");
    assert!(out.ends_with(b"\x1b[?1049l\x1b[?25h"), "terminal restored");
    assert!(
        exit_time < Duration::from_secs(3),
        "workers stopped promptly: {exit_time:?}"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn interactive_sync_runs_only_read_only_git_commands() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("tui-pty-audit");
    let repo = Repo::init(tmp.join("demo"));
    repo.write("a.txt", "a\n");
    repo.write(".gitignore", "build/\n");
    repo.commit_all("base");
    repo.write("build/out.o", "x");
    repo.write("a.txt", "changed\n");
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    let bin = tmp.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = tmp.join("calls.log");
    std::fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\nprintf '%s %s %s\\n' \"$GIT_OPTIONAL_LOCKS\" \"$3\" \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = repo.path.clone();
    let repo_for_git = repo.clone();
    let steps = vec![
        Step::Wait(800),
        Step::Run(Box::new(move || {
            std::fs::write(path.join("a.txt"), "edited again\n").unwrap()
        })),
        Step::Wait(1200),
        Step::Run(Box::new(move || {
            repo_for_git.git(&["add", "a.txt"]);
        })),
        Step::Wait(1200),
        Step::Key(b"r"),
        Step::Wait(800),
        // History: list commits, open one, load a file, go back.
        Step::Key(b"L"),
        Step::Key(b"\r"),
        Step::Key(b"]"),
        Step::Key(b"\x1b"),
        Step::Key(b"\x1b"),
        Step::Key(b"q"),
    ];
    let env_path = std::ffi::OsString::from(format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap()
    ));
    let Some((code, _out, _)) =
        pty_steps(&repo.path, &[], steps, &[("PATH", env_path.as_os_str())])
    else {
        return;
    };
    assert_eq!(code, 0);
    let calls = std::fs::read_to_string(&log).unwrap();
    let allowed = [
        "rev-parse",
        "status",
        "for-each-ref",
        "diff-index",
        "diff-files",
        "ls-files",
        "cat-file",
        "check-ignore",
        "log",
        "diff-tree",
    ];
    let mut seen = std::collections::BTreeSet::new();
    for line in calls.lines() {
        let mut parts = line.splitn(3, ' ');
        let (locks, sub, all) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap_or(""),
        );
        assert_eq!(locks, "0", "GIT_OPTIONAL_LOCKS=0 on every call: {line}");
        assert!(allowed.contains(&sub), "git {sub} is not allowed");
        if sub.starts_with("diff-") {
            assert!(
                all.contains("--no-ext-diff") && all.contains("--no-textconv"),
                "no external diff or textconv: {line}"
            );
        }
        if sub == "log" {
            assert!(all.contains("--no-show-signature"), "{line}");
        }
        seen.insert(sub.to_owned());
    }
    for needed in ["check-ignore", "diff-files", "log", "diff-tree"] {
        assert!(seen.contains(needed), "{needed} not exercised: {seen:?}");
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn termination_signals_restore_the_terminal() {
    let tmp = TempDir::new("tui-pty-signal");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    repo.write("a.txt", "a\nb\n");
    for (sig, number) in [("TERM", 15), ("HUP", 1), ("INT", 2)] {
        let steps = vec![Step::Key(b"j"), Step::Signal(sig)];
        let Some((code, out, exit_time)) = pty_steps(&repo.path, &[], steps, &[]) else {
            return;
        };
        assert_eq!(
            code,
            128 + number,
            "SIG{sig}: {}",
            String::from_utf8_lossy(&out)
        );
        assert!(
            out.ends_with(b"\x1b[?1049l\x1b[?25h"),
            "SIG{sig}: alternate screen left and cursor shown"
        );
        assert!(
            exit_time < Duration::from_secs(3),
            "SIG{sig}: {exit_time:?}"
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn history_session_in_a_real_pty() {
    let tmp = TempDir::new("tui-pty-history");
    let repo = Repo::init(tmp.join("repo"));
    repo.write("a.txt", "first\n");
    repo.commit_all("initial import");
    repo.write("a.txt", "first\nsecond\n");
    repo.commit_all("add a second line");
    repo.write("a.txt", "first\nsecond\nworking\n");
    let Some((code, out)) = pty_session(
        &repo.path,
        &[],
        &[b"L", b"\r", b"]", b"\x1b", b"j", b"\x1b", b"q"],
    ) else {
        return;
    };
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
    let text = visible_text(&out);
    for needle in [
        "HISTORY",
        "add a second line",
        "initial import",
        "(root commit)",
        "OLD · parent",
        "NEW · commit",
        "+   second",
    ] {
        assert!(drawn(&text, needle), "missing {needle:?} in {text}");
    }
    assert!(
        !drawn(&text, "never on screen"),
        "the matcher is not trivially true"
    );
    assert!(out.ends_with(b"\x1b[?1049l\x1b[?25h"));
}
