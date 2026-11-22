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
//! bytes, such as the generator's IDs with a 32-character hex suffix, 1 in 8 of
//! all IDs) is interned and gets the top bit set ("escaped"); `cmp_ids` compares
//! those by their bytes. The interner is sharded 1024 ways by a hash of the bytes,
//! each shard behind its own lock with its own byte arena, so threads rarely
//! wait for each other (ADR-029).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AO};
use std::sync::{Mutex, OnceLock};

pub type Key = u64;
const ESC: u64 = 1 << 63;
const PLACES: usize = 12;

const SHARD_BITS: u32 = 10;
const SHARDS: usize = 1 << SHARD_BITS;

/// One shard of the escaped-ID interner: IDs' bytes back to back in `arena`,
/// `spans[i]` = (start, len) of ID i, and a hash index with collision chains.
#[derive(Default)]
struct Shard {
    arena: Vec<u8>,
    spans: Vec<(u32, u32)>,
    next: Vec<u32>,             // next ID in the same hash chain, u32::MAX = end
    heads: HashMap<u64, u32>,   // full hash -> first ID with that hash
}

static INTERNER: OnceLock<Vec<Mutex<Shard>>> = OnceLock::new();
static HASHER: OnceLock<ahash::RandomState> = OnceLock::new();

fn shards() -> &'static [Mutex<Shard>] {
    INTERNER.get_or_init(|| (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect())
}

#[inline]
fn hash_bytes(b: &[u8]) -> u64 {
    // Fixed seeds: the hash only places IDs in shards and chains; equality is
    // always decided by comparing bytes.
    HASHER
        .get_or_init(|| ahash::RandomState::with_seeds(0x243F_6A88, 0x85A3_08D3, 0x1319_8A2E, 0x0370_7344))
        .hash_one(b)
}

/// Base-37 digit of each byte: `0-9` -> 1..=10, `a-z` -> 11..=36, anything else 0
/// (ADR-037: a table lookup instead of a branch per character).
static DIGIT: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut c = 0;
    while c < 10 {
        t[b'0' as usize + c] = 1 + c as u8;
        c += 1;
    }
    let mut c = 0;
    while c < 26 {
        t[b'a' as usize + c] = 11 + c as u8;
        c += 1;
    }
    t
};

/// 37^k, for padding an ID of length n with one multiply by 37^(12 - n).
static POW37: [u64; PLACES + 1] = {
    let mut t = [1u64; PLACES + 1];
    let mut k = 1;
    while k <= PLACES {
        t[k] = t[k - 1] * 37;
        k += 1;
    }
    t
};

/// Base-37 key of an ID made only of `0-9a-z`, 1 to 12 bytes; `None` otherwise.
/// Branch-free over the characters: validity is one flag checked at the end.
#[inline]
pub fn try_encode(b: &[u8]) -> Option<Key> {
    let n = b.len();
    if n == 0 || n > PLACES {
        return None;
    }
    let mut v: u64 = 0;
    let mut all = 1u8; // stays 1 only if every digit is non-zero
    for &c in b {
        let d = DIGIT[c as usize];
        all &= (d != 0) as u8;
        v = v * 37 + d as u64;
    }
    if all == 0 { None } else { Some(v * POW37[PLACES - n]) }
}

/// Exact key of any non-empty ID. (A per-thread cache in front of the interner was
/// tried and removed: it scored slower on the scorer, ADR-035.)
#[inline]
pub fn pack(b: &[u8]) -> Key {
    match try_encode(b) {
        Some(k) => k,
        None => {
            if let Some(w) = try_hex_shape(b) {
                if let Some(k) = hex_key(w) {
                    return k;
                }
            }
            pack_escaped(b)
        }
    }
}

// ---- Lock-free table for the long hex shape (ADR-047) ----
//
// An ID of exactly `[a-z]` + 32 lowercase hex digits is stored as (tag, hi, lo) in
// one slot of a fixed open-addressing table, and its key is ESC | HEXK | slot. No
// lock and no byte arena: one probe, usually one cache miss.
//
// Exactness: every ID gets exactly one key. Along an ID's probe sequence, slots
// only ever change empty -> claimed -> ready, or empty -> sealed. The first slot that
// is not ready and holding another ID decides the ID's fate, and only one thread can
// win the compare-and-swap on that slot. If it is claimed for the ID, the ID lives
// there (other threads wait for it to be ready and see it). If it is sealed (the
// table passed half full), the ID is not in the table and never will be, so every
// thread sends it to the interner.

const HEXK: u64 = 1 << 62; // interner keys use bits 0..42 only
const META_EMPTY: u64 = 0;
const META_CLAIMED: u64 = 1;
const META_SEALED: u64 = 2;
const META_READY: u64 = 3; // | tag << 8

#[derive(Default)]
struct HexSlot {
    hi: AtomicU64,
    lo: AtomicU64,
    meta: AtomicU64,
}

struct HexTable {
    slots: Vec<HexSlot>,
    mask: usize,
    used: AtomicUsize,
    limit: usize,
}

static HEXTAB: OnceLock<HexTable> = OnceLock::new();

/// Create the table with room for about `expected` long IDs. Call once, before
/// any ID is packed; without it, the first hex ID creates a default-size table.
pub fn init_hex(expected: usize) {
    let _ = HEXTAB.set(HexTable::new(expected));
}

fn hex_table() -> &'static HexTable {
    HEXTAB.get_or_init(|| HexTable::new(1 << 16))
}

impl HexTable {
    fn new(expected: usize) -> HexTable {
        let cap = (expected.max(16) * 2).next_power_of_two();
    // All-zero memory is a valid table of empty slots; the allocator hands out
    // zeroed pages lazily.
    let slots: Vec<HexSlot> = unsafe {
        let layout = std::alloc::Layout::array::<HexSlot>(cap).unwrap();
        let p = std::alloc::alloc_zeroed(layout) as *mut HexSlot;
        assert!(!p.is_null(), "hex table allocation");
        Vec::from_raw_parts(p, cap, cap)
    };
        HexTable { slots, mask: cap - 1, used: AtomicUsize::new(0), limit: cap / 2 }
    }
}

/// Key of a hex-shaped ID, or None if it must go to the interner.
#[inline]
fn hex_key(w: Wide) -> Option<Key> {
    hex_key_in(hex_table(), w)
}

#[inline]
fn hex_key_in(t: &HexTable, w: Wide) -> Option<Key> {
    let ready = META_READY | (w.tag as u64) << 8;
    let mut i = (w.mix() as usize) & t.mask;
    loop {
        let s = &t.slots[i];
        let mut m = s.meta.load(AO::Acquire);
        if m == META_EMPTY {
            let full = t.used.load(AO::Relaxed) >= t.limit;
            let to = if full { META_SEALED } else { META_CLAIMED };
            match s.meta.compare_exchange(META_EMPTY, to, AO::AcqRel, AO::Acquire) {
                Ok(_) if to == META_CLAIMED => {
                    s.hi.store(w.hi, AO::Relaxed);
                    s.lo.store(w.lo, AO::Relaxed);
                    s.meta.store(ready, AO::Release);
                    t.used.fetch_add(1, AO::Relaxed);
                    return Some(ESC | HEXK | i as u64);
                }
                Ok(_) => return None, // sealed by us
                Err(cur) => m = cur,
            }
        }
        while m == META_CLAIMED {
            std::hint::spin_loop();
            m = s.meta.load(AO::Acquire);
        }
        if m == META_SEALED {
            return None;
        }
        if m == ready && s.hi.load(AO::Relaxed) == w.hi && s.lo.load(AO::Relaxed) == w.lo {
            return Some(ESC | HEXK | i as u64);
        }
        i = (i + 1) & t.mask;
    }
}

#[inline]
fn is_hex_key(k: Key) -> bool {
    k & (ESC | HEXK) == ESC | HEXK
}

/// (tag, hi, lo) of a hex key. Its order equals the IDs' byte order: the letter
/// first, then 32 lowercase hex digits, whose ASCII order is their numeric order.
#[inline]
fn hex_parts(k: Key) -> (u8, u64, u64) {
    let t = hex_table();
    let s = &t.slots[(k & !(ESC | HEXK)) as usize];
    let m = s.meta.load(AO::Acquire);
    ((m >> 8) as u8, s.hi.load(AO::Relaxed), s.lo.load(AO::Relaxed))
}

#[inline]
fn hex_bytes(k: Key) -> [u8; 33] {
    const D: &[u8; 16] = b"0123456789abcdef";
    let (tag, hi, lo) = hex_parts(k);
    let mut out = [0u8; 33];
    out[0] = b'a' + tag - 1;
    for i in 0..16 {
        out[1 + i] = D[(hi >> (60 - 4 * i) & 15) as usize];
        out[17 + i] = D[(lo >> (60 - 4 * i) & 15) as usize];
    }
    out
}

/// Same as `pack`; kept as a separate name for click_id call sites.
#[inline]
pub fn pack_once(b: &[u8]) -> Key {
    pack(b)
}

fn pack_escaped(b: &[u8]) -> Key {
    let h = hash_bytes(b);
    let s = (h as usize) & (SHARDS - 1);
    let mut sh = shards()[s].lock().unwrap();
    let mut i = sh.heads.get(&h).copied().unwrap_or(u32::MAX);
    while i != u32::MAX {
        let (start, len) = sh.spans[i as usize];
        if &sh.arena[start as usize..(start + len) as usize] == b {
            return ESC | (s as u64) << 32 | i as u64;
        }
        i = sh.next[i as usize];
    }
    let id = sh.spans.len() as u32;
    let start = sh.arena.len() as u32;
    sh.arena.extend_from_slice(b);
    sh.spans.push((start, b.len() as u32));
    let head = sh.heads.insert(h, id).unwrap_or(u32::MAX);
    sh.next.push(head);
    ESC | (s as u64) << 32 | id as u64
}

#[inline]
pub fn is_escaped(k: Key) -> bool {
    k & ESC != 0
}

/// Bytes of a base-37 key, decoded on the stack: (buffer, length).
#[inline]
fn decode_short(k: Key) -> ([u8; PLACES], usize) {
    let mut digits = [0u8; PLACES];
    let mut v = k;
    for i in (0..PLACES).rev() {
        digits[i] = (v % 37) as u8;
        v /= 37;
    }
    let mut out = [0u8; PLACES];
    let mut n = 0;
    for &d in &digits {
        if d == 0 {
            break;
        }
        out[n] = if d <= 10 { b'0' + d - 1 } else { b'a' + d - 11 };
        n += 1;
    }
    (out, n)
}

/// Run `f` on an escaped ID's bytes under its shard's lock (one lock, no copy).
#[inline]
fn with_escaped<R>(k: Key, f: impl FnOnce(&[u8]) -> R) -> R {
    if is_hex_key(k) {
        return f(&hex_bytes(k));
    }
    let sh = shards()[((k >> 32) as usize) & (SHARDS - 1)].lock().unwrap();
    let (start, len) = sh.spans[k as u32 as usize];
    f(&sh.arena[start as usize..(start + len) as usize])
}

/// Append the ID's bytes to `out` (no allocation of its own; ADR-036).
#[inline]
pub fn append_bytes(k: Key, out: &mut Vec<u8>) {
    if is_escaped(k) {
        with_escaped(k, |b| out.extend_from_slice(b));
    } else {
        let (buf, n) = decode_short(k);
        out.extend_from_slice(&buf[..n]);
    }
}

/// The ID's bytes.
pub fn unpack(k: Key) -> Vec<u8> {
    if is_escaped(k) {
        return with_escaped(k, |b| b.to_vec());
    }
    let (buf, n) = decode_short(k);
    buf[..n].to_vec()
}

/// Byte-wise comparison of two IDs. Never holds more than one shard lock, and
/// allocates only when both sides are escaped (ADR-030).
#[inline]
pub fn cmp_ids(a: Key, b: Key) -> Ordering {
    if a == b {
        return Ordering::Equal; // keys are exact: equal keys are equal IDs
    }
    match (is_escaped(a), is_escaped(b)) {
        (false, false) => a.cmp(&b),
        (false, true) => {
            let (buf, n) = decode_short(a);
            with_escaped(b, |bb| buf[..n].cmp(bb))
        }
        (true, false) => {
            let (buf, n) = decode_short(b);
            with_escaped(a, |ab| ab.cmp(&buf[..n]))
        }
        (true, true) if is_hex_key(a) && is_hex_key(b) => hex_parts(a).cmp(&hex_parts(b)),
        (true, true) => {
            let av = unpack(a); // copied, so its lock is released before b's is taken
            with_escaped(b, |bb| av.as_slice().cmp(bb))
        }
    }
}

/// An exact ID key used where an ID only needs equality (event_id dedup, ADR-031).
/// - `tag == 0`: `lo` is the base-37 key of a short `[0-9a-z]{1,12}` ID;
/// - `tag == 1..=26`: the ID is `[a-z]` + exactly 32 lowercase hex digits (the
///   generator's long shape); `tag` is the letter, `hi:lo` the 128-bit hex value;
/// - `tag == 255`: anything else; `lo` is the sharded interner's key.
/// The shapes cannot overlap (lengths 1..=12, exactly 33, or "neither"), each is
/// injective, so equal keys <=> equal IDs, with no hashing or locking for the
/// two common shapes. All-zero (tag 0, lo 0) never occurs, so it can mark empty.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Wide {
    pub hi: u64,
    pub lo: u64,
    pub tag: u8,
}

/// Lowercase hex value of each byte (`0-9a-f`), or 0x10 for anything else
/// (ADR-037: one table lookup per character, validity checked once).
static HEX: [u8; 256] = {
    let mut t = [0x10u8; 256];
    let mut c = 0;
    while c < 10 {
        t[b'0' as usize + c] = c as u8;
        c += 1;
    }
    let mut c = 0;
    while c < 6 {
        t[b'a' as usize + c] = 10 + c as u8;
        c += 1;
    }
    t
};

#[inline]
fn try_hex_shape(b: &[u8]) -> Option<Wide> {
    if b.len() != 33 || !b[0].is_ascii_lowercase() {
        return None;
    }
    let mut hi = 0u64;
    let mut lo = 0u64;
    let mut bad = 0u8; // any 0x10 marks a non-hex byte
    for &c in &b[1..17] {
        let h = HEX[c as usize];
        bad |= h;
        hi = hi << 4 | (h & 15) as u64;
    }
    for &c in &b[17..33] {
        let h = HEX[c as usize];
        bad |= h;
        lo = lo << 4 | (h & 15) as u64;
    }
    if bad & 0x10 != 0 { None } else { Some(Wide { hi, lo, tag: 1 + (b[0] - b'a') }) }
}

/// Exact wide key of any non-empty ID.
#[inline]
pub fn wide(b: &[u8]) -> Wide {
    if let Some(k) = try_encode(b) {
        return Wide { hi: 0, lo: k, tag: 0 };
    }
    if let Some(w) = try_hex_shape(b) {
        return w;
    }
    Wide { hi: 0, lo: pack_escaped(b), tag: 255 }
}

impl Wide {
    #[inline]
    pub fn mix(&self) -> u64 {
        mix(self.lo ^ self.hi.rotate_left(23) ^ (self.tag as u64) << 56)
    }
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
            // Lengths up to 14, and every 5th ID 41 bytes like the generator's hex-suffixed IDs.
            let len = if i % 5 == 0 { 41 } else { 1 + (next() % 14) as usize };
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
        // Packing again gives the same keys.
        for (x, &k) in ids.iter().zip(&keys) {
            assert_eq!(pack(x), k);
        }
        for i in 0..ids.len() {
            assert_eq!(unpack(keys[i]), ids[i]);
            for j in (0..ids.len()).step_by(7) {
                assert_eq!(cmp_ids(keys[i], keys[j]), ids[i].cmp(&ids[j]));
                assert_eq!(keys[i] == keys[j], ids[i] == ids[j]);
            }
        }
    }

    /// Many threads interning overlapping long IDs at once must agree on every key.
    #[test]
    fn hex_table_concurrent_and_sealing_is_exact() {
        // Tiny tables, so they pass half full and start sealing while 8 threads
        // race on the same IDs: every ID must get the same answer in every thread,
        // distinct IDs distinct keys, and stored parts must round-trip.
        for (round, cap) in [8usize, 64, 1000, 5000].into_iter().enumerate() {
            let t = HexTable::new(cap);
            let mut seed: u64 = 0x9E37_79B9 + round as u64;
            let ids: Vec<Wide> = (0..4000)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let small = seed % 3 == 0; // many near-equal values
                    Wide { hi: if small { seed % 4 } else { seed }, lo: seed.rotate_left(17) % if small { 8 } else { u64::MAX }, tag: 1 + (seed % 26) as u8 }
                })
                .collect();
            let results: Vec<Vec<Option<Key>>> = std::thread::scope(|sc| {
                let hs: Vec<_> = (0..8)
                    .map(|th| {
                        let (t, ids) = (&t, &ids);
                        sc.spawn(move || {
                            // Each thread visits every ID once, in its own order (the
                            // multipliers are coprime with n = 4000, so this is a permutation).
                            let n = ids.len();
                            let mul = [1usize, 3, 7, 9, 11, 13, 17, 19][th];
                            let mut v = vec![None; n];
                            let mut seen = vec![false; n];
                            for j in 0..n {
                                let x = (j * mul + th * 97) % n;
                                assert!(!seen[x]);
                                seen[x] = true;
                                v[x] = hex_key_in(t, ids[x]);
                            }
                            v
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let mut by_key: std::collections::HashMap<Key, Wide> = std::collections::HashMap::new();
            for (j, w) in ids.iter().enumerate() {
                let first = results[0][j];
                for r in &results {
                    assert_eq!(r[j], first, "threads disagree on {w:?}");
                }
                if let Some(k) = first {
                    let i = (k & !(ESC | HEXK)) as usize;
                    let m = t.slots[i].meta.load(AO::Acquire);
                    assert_eq!(((m >> 8) as u8, t.slots[i].hi.load(AO::Relaxed), t.slots[i].lo.load(AO::Relaxed)), (w.tag, w.hi, w.lo));
                    if let Some(prev) = by_key.insert(k, *w) {
                        assert_eq!(prev, *w, "two IDs share a key");
                    }
                }
            }
        }
    }

    /// Hex keys from the global table: bytes round-trip and order is byte-wise
    /// against short, escaped and other hex IDs.
    #[test]
    fn hex_keys_roundtrip_and_order() {
        let mut ids: Vec<Vec<u8>> = vec![
            b"a00000000000000000000000000000000".to_vec(),
            b"affffffffffffffffffffffffffffffff".to_vec(),
            b"u0123456789abcdef0123456789abcdef".to_vec(),
            b"u0123456789abcdef0123456789abcdee".to_vec(),
            b"z0000000000000001000000000000000f".to_vec(),
            b"u0123456789ABCDEF0123456789abcdef".to_vec(), // upper case: interner
            b"u0123456789abcdef0123456789abcdef0".to_vec(), // 34 chars: interner
            b"u".to_vec(),
            b"u1".to_vec(),
            b"zzzzzzzzzzzz".to_vec(),
            b"b".to_vec(),
        ];
        let mut seed: u64 = 0xA076_1D64_78BD_642F;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ids.push(format!("{}{:016x}{:016x}", (b'a' + (seed % 3) as u8) as char, seed % 5, seed).into_bytes());
        }
        let keys: Vec<Key> = ids.iter().map(|b| pack(b)).collect();
        for (b, &k) in ids.iter().zip(&keys) {
            assert_eq!(&unpack(k), b);
            assert_eq!(pack(b), k);
        }
        for (a, &ka) in ids.iter().zip(&keys) {
            for (b, &kb) in ids.iter().zip(&keys) {
                assert_eq!(cmp_ids(ka, kb), a.cmp(b), "{:?} vs {:?}", String::from_utf8_lossy(a), String::from_utf8_lossy(b));
            }
        }
    }

    #[test]
    fn concurrent_interning_is_consistent() {
        let ids: Vec<Vec<u8>> = (0..20_000u32)
            .map(|i| format!("u{:x}{:032x}", i % 5_000, (i as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C)).into_bytes())
            .collect();
        let results: Vec<Vec<Key>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..8)
                .map(|t| {
                    let ids = &ids;
                    s.spawn(move || {
                        // Each thread walks the IDs from a different starting point.
                        (0..ids.len()).map(|j| pack(&ids[(j + t * 2_500) % ids.len()])).collect::<Vec<Key>>()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (t, keys) in results.iter().enumerate() {
            for (j, &k) in keys.iter().enumerate() {
                let id = &ids[(j + t * 2_500) % ids.len()];
                assert_eq!(k, pack(id));
                assert_eq!(&unpack(k), id);
            }
        }
    }

    /// `wide` is exact: equal keys if and only if equal bytes, across all shapes,
    /// including near-misses of the 33-byte hex shape (ADR-031).
    #[test]
    fn fuzz_wide_is_exact() {
        let mut seed: u64 = 0xA076_1D64_78BD_642F;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let hex = b"0123456789abcdef";
        let mut ids: Vec<Vec<u8>> = Vec::new();
        for i in 0..4000u32 {
            let mut id: Vec<u8> = Vec::new();
            match i % 8 {
                0..=2 => {
                    // well-formed long: letter + 32 lowercase hex
                    id.push(b'a' + (next() % 26) as u8);
                    for _ in 0..32 {
                        id.push(hex[(next() % 16) as usize]);
                    }
                }
                3 => {
                    // near misses: 31, 32 or 34 hex digits, a digit or uppercase prefix
                    id.push(if next() % 2 == 0 { b'E' } else { b'7' });
                    for _ in 0..(31 + next() % 4) {
                        id.push(hex[(next() % 16) as usize]);
                    }
                }
                4 => {
                    // right length, one bad character (uppercase hex or a non-hex letter)
                    id.push(b'e');
                    for _ in 0..32 {
                        id.push(hex[(next() % 16) as usize]);
                    }
                    let at = 1 + (next() % 32) as usize;
                    id[at] = if next() % 2 == 0 { id[at].to_ascii_uppercase() } else { b'g' };
                    if id[at].is_ascii_digit() {
                        id[at] = b'z';
                    }
                }
                5 => {
                    // short base-36 style
                    id.push(b'e');
                    for _ in 0..(1 + next() % 8) {
                        id.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(next() % 36) as usize]);
                    }
                }
                _ => {
                    // anything: spaces, punctuation, mixed case, any length
                    for _ in 0..(1 + next() % 40) {
                        id.push(b"aZ09 _-fF\t"[(next() % 10) as usize]);
                    }
                }
            }
            ids.push(id);
        }
        // Duplicates on purpose, so equal IDs must give equal keys.
        let n = ids.len();
        for j in 0..500 {
            let dup = ids[(j * 7) % n].clone();
            ids.push(dup);
        }
        let keys: Vec<Wide> = ids.iter().map(|x| wide(x)).collect();
        for i in 0..ids.len() {
            assert_ne!(keys[i], Wide::default(), "all-zero key must never occur");
            for j in 0..ids.len() {
                assert_eq!(keys[i] == keys[j], ids[i] == ids[j], "{:?} vs {:?}", String::from_utf8_lossy(&ids[i]), String::from_utf8_lossy(&ids[j]));
            }
        }
        // The hex shape takes the fast path; near misses do not.
        assert_eq!(wide(b"e0123456789abcdef0123456789abcdef").tag, b'e' - b'a' + 1);
        assert_eq!(wide(b"e0123456789ABCDEF0123456789abcdef").tag, 255);
        assert_eq!(wide(b"e0123456789abcdef0123456789abcde").tag, 255);
        assert_eq!(wide(b"u4wi18vd5").tag, 0);
    }

    /// The cached path (`pack`) and the uncached one (`pack_once`) give the same key
    /// for every ID, on every thread, however often it repeats (ADR-035).
    #[test]
    fn cached_pack_matches_uncached() {
        let ids: Vec<Vec<u8>> = (0..3000u32)
            .map(|i| match i % 3 {
                0 => format!("u{:032x}", (i as u128 % 700).wrapping_mul(0x9E37_79B9_7F4A_7C15_1234)).into_bytes(),
                1 => format!("u{:x}", i % 500).into_bytes(),
                _ => format!("U{:032x}", i % 300).into_bytes(), // uppercase prefix: not hex-shaped
            })
            .collect();
        std::thread::scope(|s| {
            for t in 0..6 {
                let ids = &ids;
                s.spawn(move || {
                    for round in 0..3 {
                        for j in 0..ids.len() {
                            let id = &ids[(j * 7 + t * 131 + round) % ids.len()];
                            let a = pack(id);
                            assert_eq!(a, pack_once(id));
                            assert_eq!(&unpack(a), id);
                        }
                    }
                });
            }
        });
    }
}
