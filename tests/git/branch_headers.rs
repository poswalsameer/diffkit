//! Branch and HEAD state from the status `branch.*` headers.

use diffkit::{
    git::{branches::*, status::BranchHeaders},
    types::repository::HeadState,
};

const OID: &[u8] = b"0123456789abcdef0123456789abcdef01234567";

fn headers(oid: &[u8], head: &[u8]) -> BranchHeaders {
    BranchHeaders {
        oid: Some(oid.to_vec()),
        head: Some(head.to_vec()),
        ..Default::default()
    }
}

#[test]
fn normal_branch() {
    let b = from_headers(&headers(OID, b"feature/login")).unwrap();
    assert_eq!(b.head, HeadState::Branch("feature/login".into()));
    assert_eq!(b.head_commit.as_ref().unwrap().as_str().as_bytes(), OID);
    assert!(!b.detached() && !b.unborn());
}

#[test]
fn detached_head_is_not_a_branch_named_head() {
    let b = from_headers(&headers(OID, b"(detached)")).unwrap();
    assert!(b.detached());
    assert_eq!(b.name(), None);
    assert!(b.head_commit.is_some());
}

#[test]
fn unborn_branch_has_name_but_no_commit() {
    let b = from_headers(&headers(b"(initial)", b"main")).unwrap();
    assert_eq!(b.head, HeadState::Unborn("main".into()));
    assert_eq!(b.name(), Some("main"));
    assert_eq!(b.head_commit, None);
}

#[test]
fn unknown_head() {
    let b = from_headers(&headers(OID, b"(unknown)")).unwrap();
    assert_eq!(b.head, HeadState::Unknown);
}

#[test]
fn malformed_headers_are_errors() {
    assert!(from_headers(&headers(b"xyz", b"main")).is_err());
    assert!(from_headers(&BranchHeaders::default()).is_err());
}
