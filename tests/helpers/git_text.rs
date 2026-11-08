//! Git output conventions: the line terminator and C-quoted strings.

use diffkit::helpers::git_text::{strip_newline, unquote};

#[test]
fn strip_newline_keeps_other_whitespace() {
    assert_eq!(
        strip_newline(b"dir with trailing space \n"),
        b"dir with trailing space "
    );
    assert_eq!(strip_newline(b"a\n\n"), b"a\n");
    assert_eq!(strip_newline(b"a"), b"a");
}

#[test]
fn unquote_rules() {
    assert_eq!(
        unquote(b"\"a\\\"b\\\\c\\001\"x").unwrap(),
        (b"a\"b\\c\x01".to_vec(), 13)
    );
    assert!(unquote(b"\"unterminated").is_none());
    assert!(unquote(b"\"bad \\q\"").is_none());
    assert!(unquote(b"\"bad \\4xx\"").is_none());
}
