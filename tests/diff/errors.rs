//! Diff engine error messages.

use diffkit::diff::errors::*;

#[test]
fn messages_are_actionable() {
    let e = DiffError::Stale {
        path: "a.txt".into(),
        reason: StaleReason::StatsChanged,
    };
    assert_eq!(
        e.to_string(),
        "a.txt changed since it was discovered (the added/removed line counts changed); discover again"
    );
    assert_eq!(
        DiffError::Parse {
            line: 7,
            detail: "truncated hunk".into()
        }
        .to_string(),
        "malformed diff output at line 7: truncated hunk"
    );
}
