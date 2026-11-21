//! IDs as 64-bit integers (ADR-023).
//!
//! An ID of 1 to 12 characters from `0-9a-z` is encoded in base 37: digits map to
//! 1..=10, letters to 11..=36, and the ID is right-padded with 0 to 12 places.
//! 37^12 < 2^63, so the top bit is free. The encoding is exact (equal keys <=>
//! equal IDs) and order-preserving: digits sort before letters in ASCII, and a
//! shorter ID sorts before any longer ID it is a prefix of, so comparing keys as
//! integers is the same as comparing the IDs byte-wise.
//!
//! Any other ID (upper case, spaces, punctuation, multi-byte UTF-8, more than 12
//! bytes) is interned in a shared table and gets the top bit set ("escaped");
//! `cmp_ids` compares those by their bytes.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub type Key = u64;
const ESC: u64 = 1 << 63;
const PLACES: usize = 12;

static ESCAPED: OnceLock<Mutex<(HashMap<Vec<u8>, u64>, Vec<Vec<u8>>)>> = OnceLock::new();

fn escaped_table() -> &'static Mutex<(HashMap<Vec<u8>, u64>, Vec<Vec<u8>>)> {
    ESCAPED.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

#[inline]
fn digit(c: u8) -> u64 {
    match c {
        b'0'..=b'9' => (c - b'0' + 1) as u64,
        b'a'..=b'z' => (c - b'a' + 11) as u64,
        _ => 0,
    }
}

/// Base-37 key of an ID made only of `0-9a-z`, 1 to 12 bytes; `None` otherwise.
#[inline]
pub fn try_encode(b: &[u8]) -> Option<Key> {
    if b.is_empty() || b.len() > PLACES {
        return None;
    }
    let mut v: u64 = 0;
    for &c in b {
        let d = digit(c);
        if d == 0 {
            return None;
        }
        v = v * 37 + d;
    }
    for _ in b.len()..PLACES {
        v *= 37;
    }
    Some(v)
}

/// Exact key of any non-empty ID.
#[inline]
pub fn pack(b: &[u8]) -> Key {
    match try_encode(b) {
        Some(k) => k,
        None => pack_escaped(b),
    }
}

#[cold]
fn pack_escaped(b: &[u8]) -> Key {
    let mut t = escaped_table().lock().unwrap();
    let next = t.1.len() as u64;
    let idx = *t.0.entry(b.to_vec()).or_insert(next);
    if idx == next {
        t.1.push(b.to_vec());
    }
    ESC | idx
}

#[inline]
pub fn is_escaped(k: Key) -> bool {
    k & ESC != 0
}

/// The ID's bytes.
pub fn unpack(k: Key) -> Vec<u8> {
    if is_escaped(k) {
        let t = escaped_table().lock().unwrap();
        return t.1[(k & !ESC) as usize].clone();
    }
    let mut digits = [0u8; PLACES];
    let mut v = k;
    for i in (0..PLACES).rev() {
        digits[i] = (v % 37) as u8;
        v /= 37;
    }
    digits
        .iter()
        .take_while(|&&d| d != 0)
        .map(|&d| if d <= 10 { b'0' + d - 1 } else { b'a' + d - 11 })
        .collect()
}

/// Byte-wise comparison of two IDs.
#[inline]
pub fn cmp_ids(a: Key, b: Key) -> Ordering {
    if !is_escaped(a) && !is_escaped(b) { a.cmp(&b) } else { unpack(a).cmp(&unpack(b)) }
}

/// A fast, good-enough mix of a key to 64 bits (for bucketing and hashing).
#[inline]
pub fn mix(k: Key) -> u64 {
    let x = (k ^ (k >> 31)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 29)
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
    fn write_u64(&mut self, k: u64) {
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
        let ids: Vec<&[u8]> = vec![
            b"B", b"U1", b"a", b"u1", b"u1 ", b"u1\0", b"u10", b"u2", b" u1", b"A\tB", b"0", b"9", b"z",
            b"zzzzzzzzzzzz", b"zzzzzzzzzzzzz", b"u4wi18vd5", b"u4wi18vd", b"user_id_a", b"\xc3\xa9",
        ];
        for &x in &ids {
            assert_eq!(unpack(pack(x)), x, "{:?}", x);
            for &y in &ids {
                assert_eq!(cmp_ids(pack(x), pack(y)), x.cmp(y), "{:?} {:?}", x, y);
                assert_eq!(pack(x) == pack(y), x == y);
            }
        }
        assert!(try_encode(b"zzzzzzzzzzzz").unwrap() < ESC);
        assert!(try_encode(b"U1").is_none() && try_encode(b"").is_none() && try_encode(b"abcdefghijklm").is_none());
    }

    /// Random IDs over a mixed alphabet: encoding round-trips, is exact, and
    /// preserves byte-wise order, whether or not either side is escaped.
    #[test]
    fn fuzz_order_and_roundtrip() {
        let alphabet: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzAZ _-\t";
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut ids: Vec<Vec<u8>> = Vec::new();
        for i in 0..3000 {
            let len = 1 + (next() % 14) as usize;
            let pure = i % 3 != 0; // mostly encodable
            let id: Vec<u8> = (0..len)
                .map(|_| {
                    let n = if pure { 36 } else { alphabet.len() as u64 };
                    alphabet[(next() % n) as usize]
                })
                .collect();
            ids.push(id);
        }
        let keys: Vec<Key> = ids.iter().map(|x| pack(x)).collect();
        for i in 0..ids.len() {
            assert_eq!(unpack(keys[i]), ids[i]);
            for j in (0..ids.len()).step_by(7) {
                assert_eq!(cmp_ids(keys[i], keys[j]), ids[i].cmp(&ids[j]));
                assert_eq!(keys[i] == keys[j], ids[i] == ids[j]);
            }
        }
    }
}
