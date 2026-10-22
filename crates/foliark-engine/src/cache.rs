//! Byte-accounted LRU for the per-document lazy caches (spec §9 has no cache limit, so the cap
//! is an internal constant). Sizes are estimates the caller computes, not allocator truth: the
//! point is a bound that scales with page content instead of with how many pages were visited.
//! Nothing here is persistent or shared: a cache dies with its document, and
//! `close`/`reset` free it (spec §10.2: no physical wipe is promised).
use hayro::hayro_interpret::{ImageData, ReducedImageCache, ReducedImageKey};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Mutex;

/// Per document. With `maxOpenDocuments` 4 the worst case is 4 x 16 MiB plus one oversized page each.
pub(crate) const DOC_CACHE_BYTES: usize = 16 << 20;

struct Slot<V> {
    value: V,
    bytes: usize,
    used: u64,
}

pub struct Lru<K, V> {
    slots: HashMap<K, Slot<V>>,
    cap: usize,
    bytes: usize,
    clock: u64,
    evictions: u64,
}

impl<K: Eq + Hash + Clone, V> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Self {
            slots: HashMap::new(),
            cap,
            bytes: 0,
            clock: 0,
            evictions: 0,
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.slots.contains_key(key)
    }

    /// Marks the entry most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.clock += 1;
        let slot = self.slots.get_mut(key)?;
        slot.used = self.clock;
        Some(&slot.value)
    }

    /// Store `value` (replacing the key's old entry) and evict least recently used entries until
    /// the total is within the cap. The new entry is never the one evicted, so an entry larger
    /// than the cap is kept alone and the bound is the cap plus one entry.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.clock += 1;
        if let Some(old) = self.slots.insert(
            key.clone(),
            Slot {
                value,
                bytes,
                used: self.clock,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        while self.bytes > self.cap && self.slots.len() > 1 {
            let oldest = self
                .slots
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| k.clone());
            let Some(slot) = oldest.and_then(|k| self.slots.remove(&k)) else {
                break;
            };
            self.bytes -= slot.bytes;
            self.evictions += 1;
        }
    }

    /// Change the cap; entries over the new one go at the next insert.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots.values_mut().map(|s| &mut s.value)
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.bytes = 0;
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Entries dropped to stay under the cap since creation (not counting `clear`).
    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

/// Reduced image decodes of one document (hayro P17), shared by every render of it, so a page drawn
/// again at the same size skips the decode and the downscale. Filled by the renderer through the
/// [`ReducedImageCache`] hook; never holds anything an export reads.
pub(crate) struct ImageStore(Mutex<(Lru<ReducedImageKey, ImageData>, HashSet<ReducedImageKey>)>);

/// Keys remembered as drawn once; a document draws far fewer images than this.
const SEEN_KEYS: usize = 4096;

impl ImageStore {
    pub(crate) fn new(cap: usize) -> Self {
        Self(Mutex::new((Lru::new(cap), HashSet::new())))
    }

    pub(crate) fn set_cap(&self, cap: usize) {
        if let Ok(mut s) = self.0.lock() {
            s.0.set_cap(cap);
        }
    }

    /// (bytes, entries, evictions)
    pub(crate) fn usage(&self) -> (usize, usize, u64) {
        self.0
            .lock()
            .map_or((0, 0, 0), |s| (s.0.bytes(), s.0.len(), s.0.evictions()))
    }
}

impl ReducedImageCache for ImageStore {
    fn get(&self, key: &ReducedImageKey) -> Option<ImageData> {
        self.0.lock().ok()?.0.get(key).cloned()
    }

    fn seen(&self, key: &ReducedImageKey) -> bool {
        let Ok(mut s) = self.0.lock() else {
            return false;
        };
        if s.1.len() >= SEEN_KEYS {
            s.1.clear();
        }
        !s.1.insert(*key)
    }

    fn insert(&self, key: ReducedImageKey, image: &ImageData) {
        let bytes = match image {
            ImageData::Luma(d) => d.data.len(),
            ImageData::Rgb(d) => d.data.len(),
        };
        if let Ok(mut s) = self.0.lock() {
            s.0.insert(key, image.clone(), bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Lru;

    #[test]
    fn evicts_the_least_recently_used_and_keeps_the_newest_even_when_oversize() {
        let mut c = Lru::new(100);
        c.insert("a", 1, 40);
        c.insert("b", 2, 40);
        assert_eq!(c.get(&"a"), Some(&1)); // a is now newer than b
        c.insert("c", 3, 40); // 120 > 100: b goes, not a
        let held = |c: &Lru<&str, i32>| ["a", "b", "c"].map(|k| c.contains(&k));
        assert_eq!(held(&c), [true, false, true]);
        assert_eq!((c.bytes(), c.len(), c.evictions()), (80, 2, 1));
        // Replacing a key swaps its size instead of adding to it.
        c.insert("a", 9, 10);
        assert_eq!((c.bytes(), c.get(&"a")), (50, Some(&9)));
        // One entry over the cap is served and kept alone; everything else is evicted.
        c.insert("big", 7, 500);
        assert_eq!((c.len(), c.bytes(), c.get(&"big")), (1, 500, Some(&7)));
        c.clear();
        assert_eq!((c.len(), c.bytes()), (0, 0));
    }
}
