//! Change sets: set operations and their names.

use diffkit::sync::events::*;

#[test]
fn set_operations() {
    let s = ChangeSet::WORKTREE | ChangeSet::INDEX;
    assert!(s.contains(ChangeSet::INDEX));
    assert!(!s.contains(ChangeSet::HEAD));
    assert!(s.intersects(ChangeSet::INDEX | ChangeSet::HEAD));
    assert!(s.within(ChangeSet::WORKTREE | ChangeSet::INDEX | ChangeSet::REFS));
    assert!(!s.within(ChangeSet::WORKTREE));
    assert!(ChangeSet::NONE.is_empty() && ChangeSet::NONE.within(ChangeSet::NONE));
    assert_eq!(s.names(), vec!["worktree", "index"]);
    assert_eq!(format!("{s:?}"), "ChangeSet(worktree|index)");
}
