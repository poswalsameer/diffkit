//! Byte-level conventions of Git's text output, shared by every module
//! that reads it: the line terminator and C-style quoted paths. Pure
//! functions on bytes; no Git process, no I/O.

/// Removes exactly one trailing `\n` (Git's line terminator) without
/// touching other whitespace, which may be part of a path.
pub fn strip_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

/// Decodes a C-quoted string starting at `s[0] == '"'`. Returns the bytes
/// and the number of input bytes consumed, including both quotes.
pub fn unquote(s: &[u8]) -> Option<(Vec<u8>, usize)> {
    if s.first() != Some(&b'"') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < s.len() {
        match s[i] {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                let c = *s.get(i + 1)?;
                i += 2;
                out.push(match c {
                    b'a' => 7,
                    b'b' => 8,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 11,
                    b'f' => 12,
                    b'r' => b'\r',
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'0'..=b'3' => {
                        let d = s.get(i..i + 2)?;
                        if !d.iter().all(|b| (b'0'..=b'7').contains(b)) {
                            return None;
                        }
                        i += 2;
                        (c - b'0') * 64 + (d[0] - b'0') * 8 + (d[1] - b'0')
                    }
                    _ => return None,
                });
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    None
}
