//! Safe, deterministic text for display. Raw paths and content stay bytes
//! in the models; these functions only produce what is shown.
//!
//! Nothing that reaches the terminal can contain a control character:
//! control characters, Unicode bidirectional/format controls, and invalid
//! UTF-8 bytes are replaced by visible escapes.

use std::{fmt::Write, path::Path};
use unicode_width::UnicodeWidthChar;

/// Unicode format characters that reorder or hide text (bidi overrides,
/// isolates, zero-width marks). Shown escaped so they cannot spoof a name.
pub fn is_format_control(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// Displays a path, quoting and escaping it if it would otherwise be
/// ambiguous, invisible, or unsafe in line-oriented output.
pub fn display_path(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let lossy = path.to_string_lossy();
    #[cfg(not(unix))]
    let bytes = lossy.as_bytes();

    let mut quoted = String::new();
    let mut needs_quotes = false;
    #[cfg(not(unix))]
    {
        needs_quotes |= path.to_str().is_none();
    }
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\n' => quoted.push_str("\\n"),
                '\t' => quoted.push_str("\\t"),
                '\r' => quoted.push_str("\\r"),
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                c if c.is_control() || is_format_control(c) => {
                    let _ = write!(quoted, "\\u{{{:x}}}", u32::from(c));
                }
                c => {
                    quoted.push(c);
                    continue;
                }
            }
            needs_quotes = true;
        }
        for b in chunk.invalid() {
            let _ = write!(quoted, "\\x{b:02x}");
            needs_quotes = true;
        }
    }
    let padded = quoted.starts_with(char::is_whitespace) || quoted.ends_with(char::is_whitespace);
    if needs_quotes || padded || quoted.is_empty() || quoted.contains(" -> ") {
        format!("\"{quoted}\"")
    } else {
        quoted
    }
}

/// Escapes control and format characters in short display text such as
/// branch names and error messages.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() || is_format_control(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// A piece of displayed content: either text from the file or a visible
/// escape standing in for bytes that cannot be shown as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub escaped: bool,
}

/// Display form of one line of file content.
///
/// - Tabs expand to the next multiple of `tab_width` columns.
/// - C0 controls and DEL become Unicode control pictures (`␛` for ESC).
/// - C1 controls and bidi/format controls become `<U+XXXX>`.
/// - Invalid UTF-8 bytes become `\xNN`.
///
/// Escapes are separate segments so they can be styled distinctly.
pub fn sanitize_content(bytes: &[u8], tab_width: usize) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut col = 0usize;
    let push = |out: &mut Vec<Segment>, s: &str, escaped: bool| match out.last_mut() {
        Some(last) if last.escaped == escaped => last.text.push_str(s),
        _ => out.push(Segment {
            text: s.to_owned(),
            escaped,
        }),
    };
    let mut buf = [0u8; 4];
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\t' => {
                    let n = tab_width - col % tab_width.max(1);
                    push(&mut out, &" ".repeat(n), false);
                    col += n;
                }
                '\0'..='\u{1f}' => {
                    let pic = char::from_u32(0x2400 + u32::from(c)).expect("control picture");
                    push(&mut out, pic.encode_utf8(&mut buf), true);
                    col += 1;
                }
                '\u{7f}' => {
                    push(&mut out, "\u{2421}", true);
                    col += 1;
                }
                c if c.is_control() || is_format_control(c) => {
                    let s = format!("<U+{:04X}>", u32::from(c));
                    col += s.len();
                    push(&mut out, &s, true);
                }
                c => {
                    push(&mut out, c.encode_utf8(&mut buf), false);
                    col += c.width().unwrap_or(0);
                }
            }
        }
        for b in chunk.invalid() {
            push(&mut out, &format!("\\x{b:02x}"), true);
            col += 4;
        }
    }
    out
}

/// Display width of text in terminal columns.
pub fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Fits `s` into `max` columns. Longer text keeps its end (the file name)
/// and starts with `…`.
pub fn truncate_start(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1; // the ellipsis
    for c in s.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// Fits `s` into `max` columns, cutting the end with `…`.
pub fn truncate_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 1;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
