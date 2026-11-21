//! IDs as integers.
//!
//! An ID of up to 15 bytes is packed into a `u128`: its bytes big-endian in the
//! top 15 bytes (zero-padded) and its length in the low byte. Packing is exact
//! (equal keys <=> equal IDs), and for short IDs comparing the keys as integers
//! is the same as comparing the IDs byte-wise. Longer IDs (the generator never
//! writes one) are interned in a shared table and get keys with low byte 0xFF;
//! `cmp_ids` compares those by their bytes.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub type Key = u128;
const LONG: u128 = 0xFF;

static LONG_IDS: OnceLock<Mutex<(HashMap<Vec<u8>, u64>, Vec<Vec<u8>>)>> = OnceLock::new();

fn long_table() -> &'static Mutex<(HashMap<Vec<u8>, u64>, Vec<Vec<u8>>)> {
    LONG_IDS.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

#[inline]
pub fn pack(b: &[u8]) -> Key {
    if b.len() <= 15 {
        let mut buf = [0u8; 16];
        buf[..b.len()].copy_from_slice(b);
        buf[15] = b.len() as u8;
        u128::from_be_bytes(buf)
    } else {
        pack_long(b)
    }
}

#[cold]
fn pack_long(b: &[u8]) -> Key {
    let mut t = long_table().lock().unwrap();
    let next = t.1.len() as u64;
    let idx = *t.0.entry(b.to_vec()).or_insert(next);
    if idx == next {
        t.1.push(b.to_vec());
    }
    ((idx as u128) << 8) | LONG
}

#[inline]
pub fn is_long(k: Key) -> bool {
    k & 0xFF == LONG
}

/// The ID's bytes.
pub fn unpack(k: Key) -> Vec<u8> {
    if is_long(k) {
        let t = long_table().lock().unwrap();
        t.1[(k >> 8) as usize].clone()
    } else {
        let buf = k.to_be_bytes();
        buf[..buf[15] as usize].to_vec()
    }
}

/// Byte-wise comparison of two IDs.
#[inline]
pub fn cmp_ids(a: Key, b: Key) -> Ordering {
    if !is_long(a) && !is_long(b) { a.cmp(&b) } else { unpack(a).cmp(&unpack(b)) }
}

/// A fast, good-enough mix of a key to 64 bits (for bucketing and hashing).
#[inline]
pub fn mix(k: Key) -> u64 {
    let lo = k as u64;
    let hi = (k >> 64) as u64;
    let x = (lo ^ hi.rotate_left(29)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 32)
}

/// Hasher for `HashMap<Key, _>` built on `mix`.
#[derive(Default, Clone, Copy)]
pub struct KeyHasher(u64);

impl std::hash::Hasher for KeyHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01B3);
        }
    }
    #[inline]
    fn write_u128(&mut self, k: u128) {
        self.0 = mix(k);
    }
}

pub type KeyBuild = std::hash::BuildHasherDefault<KeyHasher>;
pub type KeyMap<V> = HashMap<Key, V, KeyBuild>;

pub fn key_map<V>(cap: usize) -> KeyMap<V> {
    HashMap::with_capacity_and_hasher(cap, KeyBuild::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_bytewise() {
        let ids: Vec<&[u8]> = vec![b"B", b"U1", b"a", b"u1", b"u1 ", b"u1\0", b"u10", b"u2", b" u1", b"A\tB"];
        for &x in &ids {
            assert_eq!(unpack(pack(x)), x);
            for &y in &ids {
                assert_eq!(cmp_ids(pack(x), pack(y)), x.cmp(y), "{:?} {:?}", x, y);
            }
        }
        let long = b"a-very-long-identifier-1".as_slice();
        assert_eq!(unpack(pack(long)), long);
        assert_eq!(pack(long), pack(long));
        assert_eq!(cmp_ids(pack(long), pack(b"b")), Ordering::Less);
    }
}
