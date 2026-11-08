//! Display text: content sanitizing, escaping, path quoting, width, and
//! truncation.

use diffkit::helpers::display::*;
use std::path::Path;

fn joined(segs: &[Segment]) -> String {
    segs.iter().map(|s| s.text.as_str()).collect()
}

#[test]
fn plain_content_is_one_segment() {
    let s = sanitize_content("let x = \"caf\u{e9}\";".as_bytes(), 4);
    assert_eq!(s.len(), 1);
    assert!(!s[0].escaped);
}

#[test]
fn tabs_expand_to_tab_stops() {
    assert_eq!(joined(&sanitize_content(b"\tx", 4)), "    x");
    assert_eq!(joined(&sanitize_content(b"ab\tx", 4)), "ab  x");
    assert_eq!(joined(&sanitize_content(b"abcd\tx", 4)), "abcd    x");
}

#[test]
fn terminal_escapes_never_pass_through() {
    let s = sanitize_content(b"red \x1b[31m! bell\x07 del\x7f", 4);
    let text = joined(&s);
    assert!(!text.chars().any(|c| c.is_control()), "{text:?}");
    assert!(text.contains("\u{241b}[31m"));
    assert!(text.contains('\u{2407}') && text.contains('\u{2421}'));
    assert!(s.iter().any(|seg| seg.escaped && seg.text == "\u{241b}"));
}

#[test]
fn invalid_bytes_and_bidi_controls_are_escaped() {
    let s = sanitize_content(b"a\xffb\xe2\x80\xaec\xc2\x85", 4);
    assert_eq!(joined(&s), "a\\xffb<U+202E>c<U+0085>");
    assert!(s.iter().filter(|seg| seg.escaped).count() >= 3);
}

#[test]
fn paths_escape_format_controls() {
    assert_eq!(
        display_path(Path::new("evil\u{202e}txt.exe")),
        "\"evil\\u{202e}txt.exe\""
    );
}

#[test]
fn short_text_is_escaped() {
    assert_eq!(
        escape_text("feature/\u{202e}x\u{1b}"),
        "feature/\\u{202e}x\\u{1b}"
    );
    assert_eq!(escape_text("main"), "main");
}

#[test]
fn truncation_respects_display_width() {
    assert_eq!(truncate_start("src/deep/file.rs", 9), "…/file.rs");
    assert_eq!(truncate_start("short", 9), "short");
    assert_eq!(truncate_end("abcdef", 4), "abc…");
    assert_eq!(width("\u{65e5}\u{672c}"), 4);
    let t = truncate_start("\u{65e5}\u{672c}\u{8a9e}.rs", 6);
    assert!(width(&t) <= 6, "{t}");
    assert_eq!(truncate_start("abc", 0), "");
}

#[test]
fn plain_paths_are_unquoted() {
    assert_eq!(display_path(Path::new("src/main.rs")), "src/main.rs");
    assert_eq!(
        display_path(Path::new("dir/a file \u{65e5}.txt")),
        "dir/a file \u{65e5}.txt"
    );
}

#[test]
fn ambiguous_paths_are_quoted_and_escaped() {
    assert_eq!(display_path(Path::new("a\nb\tc")), "\"a\\nb\\tc\"");
    assert_eq!(display_path(Path::new("q\"b\\s")), "\"q\\\"b\\\\s\"");
    assert_eq!(display_path(Path::new(" lead")), "\" lead\"");
    assert_eq!(display_path(Path::new("trail ")), "\"trail \"");
    assert_eq!(display_path(Path::new("a -> b")), "\"a -> b\"");
}

#[cfg(unix)]
#[test]
fn non_utf8_bytes_are_shown_as_hex_escapes() {
    use std::os::unix::ffi::OsStrExt;
    let p = Path::new(std::ffi::OsStr::from_bytes(b"raw-\xff.txt"));
    assert_eq!(display_path(p), "\"raw-\\xff.txt\"");
}
