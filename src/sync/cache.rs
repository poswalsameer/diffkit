//! Bounded LRU cache of loaded diffs.
//!
//! Entries are keyed by comparison and file identity (old and new path,
//! never a display string). Each entry records the [`DiffFile`] metadata it
//! was loaded from: kind, modes, object IDs, working-tree stamp, line
//! counts, binary and conflict flags. After a reconciliation, an entry is
//! kept only if Git's fresh metadata for the same key is identical.
//! Otherwise it is dropped and reloaded lazily. A renamed, deleted, or
//! re-staged file, or a file that left a comparison, therefore loses its
//! entry. Unchanged files keep theirs.
//!
//! Memory is bounded by an approximate byte budget. The least recently used
//! entries are evicted first; the entry currently on screen is never
//! evicted, so the budget can be exceeded by at most that one entry. A
//! recency index keeps finding the least recently used entry independent of
//! how many entries there are.

use crate::types::diff::{DiffComparison, DiffFile, FileId};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

pub const DEFAULT_BUDGET_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub comparison: DiffComparison,
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
}

impl CacheKey {
    pub fn of(file: &DiffFile) -> Self {
        Self {
            comparison: file.comparison,
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub invalidations: u64,
    pub entries: usize,
    pub bytes: usize,
    pub peak_bytes: usize,
    pub budget_bytes: usize,
}

#[derive(Debug)]
struct Entry<V> {
    value: V,
    version: DiffFile,
    bytes: usize,
    last_used: u64,
}

/// Metadata equality ignoring the list-local [`FileId`].
pub fn same_version(a: &DiffFile, b: &DiffFile) -> bool {
    // Exhaustive, so a new field must be considered here.
    let DiffFile {
        id: _,
        comparison,
        old_path,
        new_path,
        kind,
        old_mode,
        new_mode,
        similarity,
        old_oid,
        new_oid,
        binary,
        stats,
        conflicted,
        worktree_stamp,
    } = a;
    *comparison == b.comparison
        && *old_path == b.old_path
        && *new_path == b.new_path
        && *kind == b.kind
        && *old_mode == b.old_mode
        && *new_mode == b.new_mode
        && *similarity == b.similarity
        && *old_oid == b.old_oid
        && *new_oid == b.new_oid
        && *binary == b.binary
        && *stats == b.stats
        && *conflicted == b.conflicted
        && *worktree_stamp == b.worktree_stamp
}

#[derive(Debug)]
pub struct DiffCache<V> {
    entries: HashMap<CacheKey, Entry<V>>,
    /// Each entry's `last_used` tick, oldest first.
    recency: BTreeMap<u64, CacheKey>,
    tick: u64,
    protected: Option<CacheKey>,
    stats: CacheStats,
}

impl<V> DiffCache<V> {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            recency: BTreeMap::new(),
            tick: 0,
            protected: None,
            stats: CacheStats {
                budget_bytes,
                ..CacheStats::default()
            },
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            ..self.stats
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Changes the budget, evicting least recently used entries as needed.
    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.stats.budget_bytes = budget_bytes;
        self.evict_to_budget();
    }

    /// Reads without touching recency or statistics (for rendering).
    pub fn peek(&self, key: &CacheKey) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// Looks up an entry for use, counting a hit or miss and marking it
    /// recently used.
    pub fn lookup(&mut self, key: &CacheKey) -> Option<&V> {
        self.tick += 1;
        match self.entries.get_mut(key) {
            Some(e) => {
                self.stats.hits += 1;
                let k = self.recency.remove(&e.last_used).expect("indexed");
                e.last_used = self.tick;
                self.recency.insert(self.tick, k);
                Some(&e.value)
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// The entry that must survive eviction (the one on screen).
    pub fn protect(&mut self, key: Option<CacheKey>) {
        self.protected = key;
    }

    pub fn insert(&mut self, key: CacheKey, version: &DiffFile, value: V, bytes: usize) {
        self.tick += 1;
        self.remove(&key);
        self.stats.bytes += bytes;
        self.recency.insert(self.tick, key.clone());
        self.entries.insert(
            key,
            Entry {
                value,
                version: DiffFile {
                    id: FileId(0),
                    ..version.clone()
                },
                bytes,
                last_used: self.tick,
            },
        );
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.stats.bytes);
        self.evict_to_budget();
    }

    pub fn remove(&mut self, key: &CacheKey) {
        if let Some(e) = self.entries.remove(key) {
            self.stats.bytes -= e.bytes;
            self.recency.remove(&e.last_used);
        }
    }

    fn evict_to_budget(&mut self) {
        while self.stats.bytes > self.stats.budget_bytes {
            // The protected entry is skipped; it is at most one entry.
            let victim = self
                .recency
                .values()
                .find(|k| Some(*k) != self.protected.as_ref())
                .cloned();
            let Some(k) = victim else { break };
            self.remove(&k);
            self.stats.evictions += 1;
        }
    }

    /// Keeps only entries whose file is still listed with identical
    /// metadata. `current` returns the fresh metadata for a key, if listed.
    /// Returns the number of entries invalidated.
    pub fn revalidate<'a>(&mut self, current: impl Fn(&CacheKey) -> Option<&'a DiffFile>) -> usize {
        let stale: Vec<CacheKey> = self
            .entries
            .iter()
            .filter(|(k, e)| !current(k).is_some_and(|f| same_version(&e.version, f)))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &stale {
            self.remove(k);
        }
        self.stats.invalidations += stale.len() as u64;
        stale.len()
    }

    /// Drops every entry (used when changes cannot be attributed).
    pub fn clear(&mut self) {
        self.stats.invalidations += self.entries.len() as u64;
        self.entries.clear();
        self.recency.clear();
        self.stats.bytes = 0;
    }
}
