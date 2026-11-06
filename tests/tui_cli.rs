//! The `diffkit` binary's interactive entry point: argument routing, and a
//! real pseudo-terminal session driven through `script(1)`.

mod common;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use common::numbered;
use common::{diffkit, stderr, stdout, Repo, TempDir};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

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

/// Runs `diffkit args…` inside a pseudo-terminal of 110x30, sends `keys`
/// (each after a pause), and returns (exit status, transcript bytes).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn pty_session(cwd: &Path, args: &[&str], keys: &[&[u8]]) -> Option<(i32, Vec<u8>)> {
    if Command::new("script").arg("-V").output().is_err() && !Path::new("/usr/bin/script").exists()
    {
        eprintln!("SKIPPED: script(1) is not available");
        return None;
    }
    common::isolate();
    let tmp = TempDir::new("pty");
    let transcript = tmp.join("transcript");
    let bin = env!("CARGO_BIN_EXE_diffkit");
    let quoted: Vec<String> = args.iter().map(|a| format!("'{a}'")).collect();
    let inner = format!("stty rows 30 cols 110; exec '{bin}' {}", quoted.join(" "));
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.arg("-q").arg(&transcript).args(["sh", "-c", &inner]);
    } else {
        cmd.args(["-q", "-e", "-c", &inner]).arg(&transcript);
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
    for k in keys {
        stdin.write_all(k).unwrap();
        stdin.flush().unwrap();
        std::thread::sleep(Duration::from_millis(250));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("diffkit did not exit after q");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(stdin);
    Some((
        status.code().unwrap_or(-1),
        std::fs::read(&transcript).unwrap_or_default(),
    ))
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
