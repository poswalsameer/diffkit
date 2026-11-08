//! The diff cache: hits and misses, least-recently-used eviction within
//! the byte budget, the protected entry, and targeted revalidation.

use diffkit::{
    sync::cache::*,
    types::{
        diff::{BinaryStatus, DiffComparison, DiffFile, FileChangeKind, FileId, LineStats},
        repository::ObjectId,
    },
};

fn file(c: DiffComparison, path: &str, oid: u8) -> DiffFile {
    DiffFile {
        id: FileId(7),
        comparison: c,
        old_path: Some(path.into()),
        new_path: Some(path.into()),
        kind: FileChangeKind::Modified,
        old_mode: None,
        new_mode: None,
        similarity: None,
        old_oid: ObjectId::parse(&[b'a'; 40]),
        new_oid: ObjectId::parse(&[b'0' + oid; 40]),
        binary: BinaryStatus::Text,
        stats: Some(LineStats {
            added: 1,
            removed: 0,
        }),
        conflicted: false,
        worktree_stamp: None,
    }
}

fn cache() -> DiffCache<&'static str> {
    DiffCache::new(100)
}

#[test]
fn hits_misses_and_lru_eviction() {
    let mut c = cache();
    let (a, b, d) = (
        file(DiffComparison::Staged, "a", 1),
        file(DiffComparison::Staged, "b", 1),
        file(DiffComparison::Staged, "d", 1),
    );
    assert!(c.lookup(&CacheKey::of(&a)).is_none());
    c.insert(CacheKey::of(&a), &a, "A", 40);
    c.insert(CacheKey::of(&b), &b, "B", 40);
    assert_eq!(
        c.lookup(&CacheKey::of(&a)),
        Some(&"A"),
        "a is now most recent"
    );
    c.insert(CacheKey::of(&d), &d, "D", 40);
    assert!(
        c.peek(&CacheKey::of(&b)).is_none(),
        "b was least recently used"
    );
    assert!(c.peek(&CacheKey::of(&a)).is_some() && c.peek(&CacheKey::of(&d)).is_some());
    let s = c.stats();
    assert_eq!(
        (s.hits, s.misses, s.evictions, s.entries, s.bytes),
        (1, 1, 1, 2, 80)
    );
    assert_eq!(s.peak_bytes, 120);
}

#[test]
fn budget_is_enforced_but_protected_entry_survives() {
    let mut c = cache();
    let (a, big) = (
        file(DiffComparison::Unstaged, "a", 1),
        file(DiffComparison::Unstaged, "big", 1),
    );
    c.insert(CacheKey::of(&a), &a, "A", 30);
    c.protect(Some(CacheKey::of(&big)));
    c.insert(CacheKey::of(&big), &big, "BIG", 150);
    assert!(
        c.peek(&CacheKey::of(&big)).is_some(),
        "on-screen entry kept"
    );
    assert!(
        c.peek(&CacheKey::of(&a)).is_none(),
        "others evicted to make room"
    );
    assert_eq!(c.stats().bytes, 150);
    c.protect(None);
    let b = file(DiffComparison::Unstaged, "b", 1);
    c.insert(CacheKey::of(&b), &b, "B", 10);
    assert!(
        c.peek(&CacheKey::of(&big)).is_none(),
        "evictable once unprotected"
    );
    assert!(c.stats().bytes <= 100);
}

#[test]
fn revalidation_is_targeted() {
    let mut c = cache();
    let same = file(DiffComparison::Unstaged, "same", 1);
    let changed = file(DiffComparison::Unstaged, "changed", 1);
    let gone = file(DiffComparison::Unstaged, "gone", 1);
    let staged = file(DiffComparison::Staged, "same", 1);
    for f in [&same, &changed, &gone, &staged] {
        c.insert(CacheKey::of(f), f, "v", 1);
    }
    // Fresh metadata: `same` unchanged (new list position), `changed`
    // has a new object ID, `gone` and the staged entry are not listed.
    let mut same_now = same.clone();
    same_now.id = FileId(0);
    let changed_now = file(DiffComparison::Unstaged, "changed", 2);
    let fresh = [same_now, changed_now];
    let dropped = c.revalidate(|k| fresh.iter().find(|f| CacheKey::of(f) == *k));
    assert_eq!(dropped, 3);
    assert!(c.peek(&CacheKey::of(&same)).is_some());
    assert_eq!(c.len(), 1);
    assert_eq!(c.stats().invalidations, 3);
}

#[test]
fn rename_identity_and_comparison_are_part_of_the_key() {
    let mut c = cache();
    let mut renamed = file(DiffComparison::Staged, "old", 1);
    renamed.new_path = Some("new".into());
    c.insert(CacheKey::of(&renamed), &renamed, "R", 1);
    let as_add = DiffFile {
        old_path: None,
        ..renamed.clone()
    };
    assert!(
        c.peek(&CacheKey::of(&as_add)).is_none(),
        "different identity"
    );
    let other_mode = DiffFile {
        comparison: DiffComparison::Combined,
        ..renamed.clone()
    };
    assert!(c.peek(&CacheKey::of(&other_mode)).is_none());
    assert_eq!(c.revalidate(|_| None), 1, "rename no longer listed");
}

#[test]
fn full_invalidation() {
    let mut c = cache();
    let a = file(DiffComparison::Staged, "a", 1);
    c.insert(CacheKey::of(&a), &a, "A", 10);
    c.clear();
    assert!(c.is_empty());
    assert_eq!((c.stats().bytes, c.stats().invalidations), (0, 1));
}

/// Least-recently-used eviction checked against a plain model of the
/// cache after every operation: which entries remain, and their bytes.
#[test]
fn eviction_follows_recency_through_every_operation() {
    let budget = 100;
    let mut c = DiffCache::new(budget);
    let files: Vec<DiffFile> = (0..8)
        .map(|i| file(DiffComparison::Unstaged, &format!("f{i}"), 1))
        .collect();
    // (file index, bytes, last use), and the protected file.
    let mut model: Vec<(usize, usize, u64)> = Vec::new();
    let mut protected: Option<usize> = None;
    let mut tick = 0;
    for round in 0..300usize {
        let n = round * 7 % files.len();
        let key = CacheKey::of(&files[n]);
        tick += 1;
        match round % 5 {
            0 | 1 => {
                let bytes = 10 + round % 30;
                c.insert(key, &files[n], "v", bytes);
                model.retain(|e| e.0 != n);
                model.push((n, bytes, tick));
                while model.iter().map(|e| e.1).sum::<usize>() > budget {
                    let Some(victim) = model
                        .iter()
                        .filter(|e| Some(e.0) != protected)
                        .min_by_key(|e| e.2)
                        .map(|e| e.0)
                    else {
                        break;
                    };
                    model.retain(|e| e.0 != victim);
                }
            }
            2 => {
                let hit = c.lookup(&key).is_some();
                assert_eq!(hit, model.iter().any(|e| e.0 == n), "round {round}");
                if let Some(e) = model.iter_mut().find(|e| e.0 == n) {
                    e.2 = tick;
                }
            }
            3 => {
                c.remove(&key);
                model.retain(|e| e.0 != n);
            }
            _ => {
                c.protect(Some(key));
                protected = Some(n);
            }
        }
        for (i, f) in files.iter().enumerate() {
            assert_eq!(
                c.peek(&CacheKey::of(f)).is_some(),
                model.iter().any(|e| e.0 == i),
                "round {round}, file {i}"
            );
        }
        assert_eq!(c.len(), model.len(), "round {round}");
        assert_eq!(c.stats().bytes, model.iter().map(|e| e.1).sum::<usize>());
    }
}

#[test]
fn reinsert_replaces_size() {
    let mut c = cache();
    let a = file(DiffComparison::Staged, "a", 1);
    c.insert(CacheKey::of(&a), &a, "A", 10);
    c.insert(CacheKey::of(&a), &a, "A2", 20);
    assert_eq!((c.len(), c.stats().bytes), (1, 20));
}
