//! A small open-addressing hash table for non-zero u64 keys (ADR-025).
//!
//! Two flat arrays (keys, values), power-of-two capacity, linear probing, key 0
//! marks an empty slot (no packed ID is ever 0). No deletion. It grows when 3/4
//! full, so inserts always terminate. Narrower than `std::collections::HashMap`,
//! and so simpler and more cache-friendly for our keys.

use crate::ids::mix;

pub struct FlatMap<V: Copy + Default> {
    keys: Vec<u64>,
    vals: Vec<V>,
    mask: usize,
    len: usize,
}

impl<V: Copy + Default> FlatMap<V> {
    pub fn with_capacity(n: usize) -> Self {
        let cap = (n.max(8) * 2).next_power_of_two();
        FlatMap { keys: vec![0; cap], vals: vec![V::default(); cap], mask: cap - 1, len: 0 }
    }

    #[inline]
    fn start(&self, k: u64) -> usize {
        // The low bits of the mix; partition buckets use bits 28..36, so they do not
        // correlate with the slot.
        (mix(k) as usize) & self.mask
    }

    #[inline]
    pub fn get(&self, k: u64) -> Option<V> {
        debug_assert!(k != 0);
        let mut i = self.start(k);
        loop {
            let s = self.keys[i];
            if s == k {
                return Some(self.vals[i]);
            }
            if s == 0 {
                return None;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// If `k` is present, call `update` on its value; otherwise insert `v`.
    #[inline]
    pub fn upsert(&mut self, k: u64, v: V, update: impl FnOnce(&mut V)) {
        debug_assert!(k != 0);
        let mut i = self.start(k);
        loop {
            let s = self.keys[i];
            if s == k {
                update(&mut self.vals[i]);
                return;
            }
            if s == 0 {
                self.keys[i] = k;
                self.vals[i] = v;
                self.len += 1;
                if self.len * 4 > self.keys.len() * 3 {
                    self.grow();
                }
                return;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// Insert only if absent (first value wins). Returns true if inserted.
    #[inline]
    pub fn insert_new(&mut self, k: u64, v: V) -> bool {
        let before = self.len;
        self.upsert(k, v, |_| {});
        self.len > before
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Values of all entries, in slot order.
    pub fn values(&self) -> impl Iterator<Item = V> + '_ {
        self.keys.iter().zip(&self.vals).filter(|(k, _)| **k != 0).map(|(_, v)| *v)
    }

    #[cold]
    fn grow(&mut self) {
        let mut bigger = FlatMap::with_capacity(self.keys.len());
        for (&k, &v) in self.keys.iter().zip(&self.vals) {
            if k != 0 {
                bigger.upsert(k, v, |_| {});
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
