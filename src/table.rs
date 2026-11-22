//! A small open-addressing hash table (ADR-025), generic over the key (ADR-031).
//!
//! One flat array of (key, value) slots, power-of-two capacity, linear probing, key 0
//! marks an empty slot (no packed ID is ever 0). No deletion. It grows when 3/4
//! full, so inserts always terminate. Narrower than `std::collections::HashMap`,
//! and so simpler and more cache-friendly for our keys.

use crate::ids::{mix, Wide};

/// A key the table can hold: it has a reserved "empty" value that no real key
/// takes, and a well-mixed 64-bit hash.
pub trait SlotKey: Copy + Eq + Default {
    fn is_empty(&self) -> bool;
    fn hash64(&self) -> u64;
}

impl SlotKey for u64 {
    #[inline]
    fn is_empty(&self) -> bool {
        *self == 0 // no packed ID is 0
    }
    #[inline]
    fn hash64(&self) -> u64 {
        mix(*self)
    }
}

impl SlotKey for Wide {
    #[inline]
    fn is_empty(&self) -> bool {
        self.tag == 0 && self.lo == 0 // never produced by ids::wide (ADR-031)
    }
    #[inline]
    fn hash64(&self) -> u64 {
        self.mix()
    }
}

/// The table over u64 keys, used for products, campaigns, persons.
pub type FlatMap<V> = Table<u64, V>;
/// The table over exact wide keys, used for event dedup (ADR-031).
#[allow(dead_code)]
pub type WideMap<V> = Table<Wide, V>;

/// One slot: the key next to its value, so a lookup touches one cache line (ADR-028).
#[derive(Clone, Copy, Default)]
struct Entry<K: SlotKey, V: Copy + Default> {
    k: K,
    v: V,
}

pub struct Table<K: SlotKey, V: Copy + Default> {
    slots: Vec<Entry<K, V>>,
    mask: usize,
    len: usize,
}

impl<K: SlotKey, V: Copy + Default> Table<K, V> {
    pub fn with_capacity(n: usize) -> Self {
        let cap = (n.max(8) * 2).next_power_of_two();
        Table { slots: vec![Entry::default(); cap], mask: cap - 1, len: 0 }
    }

    #[inline]
    fn start(&self, k: K) -> usize {
        // The low bits of the mix; partition buckets use bits 28..36, so they do not
        // correlate with the slot.
        (k.hash64() as usize) & self.mask
    }

    /// Hint the CPU to start loading `k`'s first slot (ADR-028). Only a hint: it
    /// never changes results. A no-op where no prefetch instruction is used.
    #[inline(always)]
    pub fn prefetch(&self, k: K) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
            let p = self.slots.as_ptr().add(self.start(k)) as *const i8;
            _mm_prefetch::<_MM_HINT_T0>(p);
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = k;
    }

    #[inline]
    pub fn get(&self, k: K) -> Option<V> {
        debug_assert!(!k.is_empty());
        let mut i = self.start(k);
        loop {
            let s = self.slots[i];
            if s.k == k {
                return Some(s.v);
            }
            if s.k.is_empty() {
                return None;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// If `k` is present, call `update` on its value; otherwise insert `v`.
    #[inline]
    pub fn upsert(&mut self, k: K, v: V, update: impl FnOnce(&mut V)) {
        debug_assert!(!k.is_empty());
        let mut i = self.start(k);
        loop {
            let s = &mut self.slots[i];
            if s.k == k {
                update(&mut s.v);
                return;
            }
            if s.k.is_empty() {
                s.k = k;
                s.v = v;
                self.len += 1;
                if self.len * 4 > self.slots.len() * 3 {
                    self.grow();
                }
                return;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// Insert only if absent (first value wins). Returns true if inserted.
    #[inline]
    pub fn insert_new(&mut self, k: K, v: V) -> bool {
        let before = self.len;
        self.upsert(k, v, |_| {});
        self.len > before
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Values of all entries, in slot order.
    pub fn values(&self) -> impl Iterator<Item = V> + '_ {
        self.slots.iter().filter(|s| !s.k.is_empty()).map(|s| s.v)
    }

    #[cold]
    fn grow(&mut self) {
        let mut bigger = Table::with_capacity(self.slots.len());
        for s in &self.slots {
            if !s.k.is_empty() {
                bigger.upsert(s.k, s.v, |_| {});
            }
        }
        *self = bigger;
    }
}

#[cfg(test)]
mod tests {
    use super::FlatMap;
    use std::collections::HashMap;

    /// Random upserts and lookups, including growth from a tiny capacity, must
    /// match std's HashMap exactly.
    #[test]
    fn fuzz_matches_std_hashmap() {
        let mut seed: u64 = 0x94D0_49BB_1331_11EB;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..20 {
            let mut ours: FlatMap<u64> = FlatMap::with_capacity(if round % 2 == 0 { 4 } else { 5000 });
            let mut std: HashMap<u64, u64> = HashMap::new();
            let universe = 1 + (next() % 20_000);
            for _ in 0..50_000 {
                let k = 1 + next() % universe; // never 0
                let v = next();
                match next() % 3 {
                    0 => {
                        // keep the minimum, like dedup by smallest ingest_ts
                        ours.upsert(k, v, |cur| *cur = (*cur).min(v));
                        let e = std.entry(k).or_insert(v);
                        *e = (*e).min(v);
                    }
                    1 => {
                        assert_eq!(ours.insert_new(k, v), !std.contains_key(&k));
                        std.entry(k).or_insert(v);
                    }
                    _ => assert_eq!(ours.get(k), std.get(&k).copied()),
                }
            }
            assert_eq!(ours.len(), std.len());
            let mut a: Vec<u64> = ours.values().collect();
            let mut b: Vec<u64> = std.values().copied().collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b);
            for k in 1..=universe.min(3000) {
                assert_eq!(ours.get(k), std.get(&k).copied());
            }
        }
    }
}
