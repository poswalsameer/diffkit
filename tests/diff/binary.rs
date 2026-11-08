//! Binary detection from file content.

use diffkit::diff::binary::*;

#[test]
fn nul_within_first_8000_bytes_is_binary() {
    assert!(!looks_binary(b"plain text\n"));
    assert!(looks_binary(b"a\0b"));
    let mut late = vec![b'a'; SNIFF_BYTES];
    late.push(0);
    assert!(
        !looks_binary(&late),
        "Git only inspects the first 8000 bytes"
    );
    assert!(
        !looks_binary(&[0xff, 0xfe, 0x80]),
        "non-UTF-8 is not binary"
    );
}
