//! Events: parse -> dedup on event_id (smallest ingest_ts, BEFORE validation)
//! -> validate -> drop late -> compact records keyed by person.

use crate::csv;
use crate::ids::{self, Key};
use crate::table::FlatMap;
use crate::ts;
use crate::Ctx;
use rayon::prelude::*;

pub const VIEW: u8 = 1;
pub const CART: u8 = 2;
pub const PURCHASE: u8 = 3;

const BUCKETS: usize = crate::partition::PARTS;
const BAD_TS: i64 = i64::MIN;
const NO_PRODUCT: u16 = u16::MAX;
const MAX_PRICE: u32 = 10_000_000;

/// One parsed copy of an event. Validity is decided here, per copy, because the
/// rules validate whichever copy wins dedup (ADR-015). The event_id is an exact
/// wide key (ADR-031), stored as separate fields, with etype and the valid flag
/// sharing a byte. The event time is stored as a u32 offset from the window
/// start, and only for valid rows (the only rows that use it); the record is
/// packed to 4-byte alignment: 44 bytes (ADR-044).
#[derive(Clone, Copy)]
#[repr(C, packed(4))]
struct RawEv {
    eid_hi: u64,
    eid_lo: u64,
    user: Key,   // 0 = null (a non-empty ID's key is never 0)
    ingest: i64, // dedup order: smallest wins
    ts: u32, // ts - window_start if valid, else 0
    price: u32, // purchase price (0 for view / cart)
    cat: u16,
    eid_tag: u8,
    etv: u8, // bits 0-1: event type; bit 7: passes every rule check, including "not late"
}

impl RawEv {
    #[inline]
    fn eid(&self) -> ids::Wide {
        ids::Wide { hi: self.eid_hi, lo: self.eid_lo, tag: self.eid_tag }
    }
    #[inline]
    fn etype(&self) -> u8 {
        self.etv & 3
    }
    #[inline]
    fn valid(&self) -> bool {
        self.etv & 0x80 != 0
    }
}

/// A surviving event: 24 bytes (ADR-023). `ts` is milliseconds since window_start,
/// which fits in 32 bits for any window under 49 days (checked in main).
#[derive(Clone, Copy)]
pub struct Ev {
    pub person: Key,
    pub ts: u32,
    pub price: u32, // purchase price, 0 for view / cart
    pub cat: u16,
    pub etype: u8,
}

#[inline]
fn parse_price(b: Option<&[u8]>) -> u32 {
    let Some(b) = b else { return 0 };
    let mut v: u64 = 0;
    let mut sig = 0;
    for &c in b {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return 0;
        }
        if sig > 0 || d != 0 {
            sig += 1;
            if sig > 8 {
                return 0;
            }
            v = v * 10 + d as u64;
        }
    }
    if (1..=MAX_PRICE as u64).contains(&v) { v as u32 } else { 0 }
}

#[inline]
fn event_type(b: Option<&[u8]>) -> u8 {
    match b {
        Some(b"view") => VIEW,
        Some(b"cart") => CART,
        Some(b"purchase") => PURCHASE,
        _ => 0,
    }
}

/// Software write-combining (ADR-053): records per bucket staged before a flush.
const WC_K: usize = 16;

/// Per-thread staging area: WC_K records for each of the BUCKETS buckets, reused
/// across chunks so it stays in the core's L2 cache.
struct Stage {
    recs: Vec<std::mem::MaybeUninit<RawEv>>, // BUCKETS * WC_K
    cnt: Vec<u8>,                             // records staged per bucket
}

thread_local! {
    static STAGE: std::cell::RefCell<Stage> = std::cell::RefCell::new(Stage {
        recs: vec![std::mem::MaybeUninit::uninit(); BUCKETS * WC_K],
        cnt: vec![0; BUCKETS],
    });
}

/// Append `src` to `dst`. With `nt`, through non-temporal stores, which write
/// to memory without first reading each destination cache line (ADR-053).
#[inline(always)]
fn flush(dst: &mut Vec<RawEv>, src: &[std::mem::MaybeUninit<RawEv>], nt: bool) {
    let n = src.len();
    dst.reserve(n);
    // SAFETY: `reserve` made room for n more records; the n staged records were written.
    unsafe {
        let d = dst.as_mut_ptr().add(dst.len());
        let s = src.as_ptr() as *const RawEv;
        if nt {
            nt_copy(d, s, n);
        } else {
            std::ptr::copy_nonoverlapping(s, d, n);
        }
        dst.set_len(dst.len() + n);
    }
}

/// Copy n records with 4-byte streaming stores (RawEv is 4-byte aligned, 44 bytes).
/// The CPU combines consecutive streaming stores into whole cache-line writes.
#[inline(always)]
unsafe fn nt_copy(d: *mut RawEv, s: *const RawEv, n: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::_mm_stream_si32;
        let words = n * std::mem::size_of::<RawEv>() / 4;
        let (d, s) = (d as *mut i32, s as *const i32);
        for w in 0..words {
            _mm_stream_si32(d.add(w), s.add(w).read());
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        std::ptr::copy_nonoverlapping(s, d, n)
    };
}

/// Write-combining mode from `SS_WC` (0 = direct push, 1 = staged + memcpy,
/// 2 = staged + streaming stores), read once. Default 1, the fastest on the VM.
fn wc_mode() -> u8 {
    static M: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *M.get_or_init(|| std::env::var("SS_WC").ok().and_then(|v| v.parse().ok()).unwrap_or(1))
}

fn parse_chunk(chunk: &[u8], ctx: &Ctx) -> Vec<Vec<RawEv>> {
    match wc_mode() {
        1 => parse_chunk_wc::<false>(chunk, ctx),
        2 => parse_chunk_wc::<true>(chunk, ctx),
        _ => {
            let est = chunk.len() / 70 / BUCKETS + 16;
            let mut b: Vec<Vec<RawEv>> = (0..BUCKETS).map(|_| Vec::with_capacity(est)).collect();
            parse_lines(chunk, ctx, |bk, r| b[bk].push(r));
            b
        }
    }
}

/// parse_chunk through the per-thread staging area: each record goes to its
/// bucket's small staging slot, and a full slot is copied out in one burst.
/// Records keep their (chunk, position) order within each bucket.
fn parse_chunk_wc<const NT: bool>(chunk: &[u8], ctx: &Ctx) -> Vec<Vec<RawEv>> {
    let est = chunk.len() / 70 / BUCKETS + 16;
    let mut b: Vec<Vec<RawEv>> = (0..BUCKETS).map(|_| Vec::with_capacity(est)).collect();
    STAGE.with(|st| {
        let st = &mut *st.borrow_mut();
        let (recs, cnt) = (&mut st.recs[..], &mut st.cnt[..]);
        parse_lines(chunk, ctx, |bk, r| {
            // SAFETY: bk < BUCKETS (bucket_wide masks it) and c < WC_K.
            unsafe {
                let c = *cnt.get_unchecked(bk) as usize;
                recs.get_unchecked_mut(bk * WC_K + c).write(r);
                if c + 1 == WC_K {
                    flush(b.get_unchecked_mut(bk), recs.get_unchecked(bk * WC_K..bk * WC_K + WC_K), NT);
                    *cnt.get_unchecked_mut(bk) = 0;
                } else {
                    *cnt.get_unchecked_mut(bk) = (c + 1) as u8;
                }
            }
        });
        for bk in 0..BUCKETS {
            let c = cnt[bk] as usize;
            if c > 0 {
                flush(&mut b[bk], &recs[bk * WC_K..bk * WC_K + c], false);
                cnt[bk] = 0;
            }
        }
    });
    if NT {
        // Streaming stores are weakly ordered: make them visible before the
        // buckets are handed to other threads.
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::x86_64::_mm_sfence()
        };
    }
    b
}

/// Parse every line of a chunk; hand each record and its eid bucket to `sink`.
#[inline(always)]
fn parse_lines(chunk: &[u8], ctx: &Ctx, mut sink: impl FnMut(usize, RawEv)) {
    let (ws, we) = (ctx.window_start, ctx.window_end);
    csv::for_each_line::<7>(chunk, |f| {
        let Some(eid) = f[0] else { return };
        let eid = ids::wide(eid);
        let etype = event_type(f[4]);
        let ts = f[2].and_then(ts::parse_ts).unwrap_or(BAD_TS);
        let ingest = f[3].map_or(i64::MAX, ts::parse_ingest);
        let price = if etype == PURCHASE { parse_price(f[6]) } else { 0 };
        let cat = match f[5] {
            None => NO_PRODUCT,
            Some(p) => ctx.category_of(p),
        };
        let valid = f[1].is_some()
            && ts != BAD_TS
            && ts >= ws
            && ts < we
            && etype != 0
            && cat != NO_PRODUCT
            && (etype != PURCHASE || price != 0)
            && ingest.saturating_sub(ts) <= ts::MS_PER_DAY;
        let r = RawEv {
            eid_hi: eid.hi,
            eid_lo: eid.lo,
            user: f[1].map_or(0, ids::pack),
            ingest,
            ts: if valid { (ts - ws) as u32 } else { 0 },
            price,
            cat,
            eid_tag: eid.tag,
            etv: etype | (valid as u8) << 7,
        };
        sink(crate::partition::bucket_wide(&eid), r);
    });
}

/// Fingerprint of an event_id's hash for the dedup table: 28 high bits (the slot
/// uses the low bits and the bucket bits 28..36), never 0.
#[inline(always)]
fn fingerprint(h: u64) -> u32 {
    ((h >> 36) as u32) | 1
}

#[inline(always)]
fn prefetch_u64(t: &[u64], s: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
        _mm_prefetch::<_MM_HINT_T0>(t.as_ptr().add(s) as *const i8);
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (t, s);
}

/// Exact dedup of one bucket's records (ADR-040). Keeps, per event_id, the copy
/// with the smallest ingest_ts, the first one seen on ties (equal copies are
/// identical). Returns a bitset over the records in (piece, position) order.
///
/// Slots are 8 bytes: `fingerprint << 32 | record index` (0 = empty; a fingerprint
/// is never 0), at load ≤ 0.625, so a bucket of about 80k records needs 1 MB, not
/// 8 MB. A fingerprint match only says "maybe": the full Wide event_ids are
/// compared, so different IDs with equal fingerprints just keep probing.
fn dedup_bucket<F: Fn(u64) -> u32>(pieces: &[&[RawEv]], fp_of: F) -> Vec<u64> {
    let n: usize = pieces.iter().map(|p| p.len()).sum();
    let mut first = Vec::with_capacity(pieces.len() + 1);
    let mut acc = 0usize;
    for p in pieces {
        first.push(acc);
        acc += p.len();
    }
    first.push(acc);
    // Record by its index: the last piece starting at or before it (empty pieces
    // share a start with the next piece, which is the one that holds it).
    let rec = |i: u32| -> &RawEv {
        let i = i as usize;
        let ci = first.partition_point(|&s| s <= i) - 1;
        &pieces[ci][i - first[ci]]
    };
    let cap = (n * 8 / 5).max(16).next_power_of_two();
    let mask = cap - 1;
    let mut t = vec![0u64; cap];
    let mut i: u32 = 0;
    for p in pieces {
        for (pos, r) in p.iter().enumerate() {
            if let Some(a) = p.get(pos + 16) {
                prefetch_u64(&t, (a.eid().mix() as usize) & mask);
            }
            let eid = r.eid();
            let h = eid.mix();
            let f = fp_of(h);
            let mut s = (h as usize) & mask;
            loop {
                let x = t[s];
                if x == 0 {
                    t[s] = (f as u64) << 32 | i as u64;
                    break;
                }
                if (x >> 32) as u32 == f {
                    let o = rec(x as u32);
                    if o.eid() == eid {
                        if r.ingest < o.ingest {
                            t[s] = (f as u64) << 32 | i as u64;
                        }
                        break;
                    }
                }
                s = (s + 1) & mask;
            }
            i += 1;
        }
    }
    let mut win = vec![0u64; n / 64 + 1];
    for &x in &t {
        if x != 0 {
            let j = x as u32 as usize;
            win[j / 64] |= 1 << (j % 64);
        }
    }
    win
}

/// Parsed copies of every event, per chunk and eid bucket. Opaque to callers.
pub struct Parsed(Vec<Vec<Vec<RawEv>>>);

/// Parse every event line. Needs no person mapping, so it runs while links are
/// being resolved (ADR-026).
pub fn parse(src: &crate::input::Source, ctx: &Ctx) -> Parsed {
    let t0 = std::time::Instant::now();
    let parsed = Parsed(src.par_map(|c| parse_chunk(c, ctx)));
    eprintln!("    events parse   {:.3}s", t0.elapsed().as_secs_f64());
    parsed
}

/// Dedup, validate, and route each surviving event to its person partition.
/// Returns `routed[eid_bucket][person_partition]`.
pub fn dedup_route(parsed: Parsed, ctx: &Ctx, persons: &FlatMap<Key>) -> Vec<Vec<Vec<Ev>>> {
    let parsed = parsed.0;
    let t1 = std::time::Instant::now();
    let parts = crate::partition::PARTS;

    // Per eid bucket: dedup by location (ADR-015), then route the valid winners.
    // Each bucket owns its pieces and frees them when done, so later buckets' output
    // reuses that memory instead of fresh pages (ADR-048).
    let routed: Vec<Vec<Vec<Ev>>> = crate::partition::transpose(parsed)
        .into_par_iter()
        .map(|bucket| {
            let n: usize = bucket.iter().map(|p| p.len()).sum();
            const AHEAD: usize = 16; // prefetch distance, in records (ADR-028)
            let pieces: Vec<&[RawEv]> = bucket.iter().map(|p| p.as_slice()).collect();
            // Winning copy of each event_id, one bit per record in (chunk, position)
            // order (ADR-027), from a cache-sized fingerprint table (ADR-040).
            let win = dedup_bucket(&pieces, fingerprint);
            let est = n / parts * 5 / 4 + 4;
            let mut out: Vec<Vec<Ev>> = (0..parts).map(|_| Vec::with_capacity(est)).collect();
            let mut i = 0usize;
            for recs in &bucket {
                for (pos, r) in recs.iter().enumerate() {
                    if let Some(a) = recs.get(pos + AHEAD) {
                        if a.valid() {
                            persons.prefetch(a.user);
                        }
                    }
                    if win[i / 64] >> (i % 64) & 1 == 1 && r.valid() {
                        let user = r.user;
                        let person = persons.get(user).unwrap_or(user);
                        out[crate::partition::bucket(person)].push(Ev {
                            person,
                            ts: r.ts,
                            price: r.price,
                            cat: r.cat,
                            etype: r.etype(),
                        });
                    }
                    i += 1;
                }
            }
            out
        })
        .collect();
    eprintln!("    events dedup+route {:.3}s", t1.elapsed().as_secs_f64());
    routed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(eid: ids::Wide, ingest: i64) -> RawEv {
        RawEv { eid_hi: eid.hi, eid_lo: eid.lo, user: 1, ingest, ts: 0, price: 0, cat: 0, eid_tag: eid.tag, etv: 1 }
    }

    /// dedup_bucket keeps exactly the reference winners (smallest ingest, first seen on
    /// ties), including with fingerprints forced to collide (ADR-040).
    #[test]
    fn fuzz_dedup_bucket_matches_reference() {
        let mut seed: u64 = 0x2F3B_9D44_77AC_1E05;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..60 {
            let universe = 1 + next() % 3000;
            let n = (next() % 6000) as usize;
            let recs: Vec<RawEv> = (0..n)
                .map(|_| {
                    let k = next() % universe;
                    let eid = match k % 3 {
                        0 => ids::Wide { hi: 0, lo: 1 + k, tag: 0 },
                        1 => ids::Wide { hi: k.wrapping_mul(0x9E37_79B9), lo: k, tag: 5 },
                        _ => ids::Wide { hi: 0, lo: (1 << 63) | k, tag: 255 },
                    };
                    raw(eid, (next() % 50) as i64) // few distinct ingests -> many ties
                })
                .collect();
            // Random split into pieces, some empty.
            let mut pieces: Vec<&[RawEv]> = Vec::new();
            let mut at = 0;
            while at < recs.len() {
                let len = if next() % 5 == 0 { 0 } else { (next() % 700) as usize };
                let end = (at + len).min(recs.len());
                pieces.push(&recs[at..end]);
                at = end;
            }
            pieces.push(&recs[recs.len()..]);
            let mut best: std::collections::HashMap<ids::Wide, (i64, usize)> = std::collections::HashMap::new();
            for (i, r) in recs.iter().enumerate() {
                let e = best.entry(r.eid()).or_insert((r.ingest, i));
                if r.ingest < e.0 {
                    *e = (r.ingest, i);
                }
            }
            let mut want = vec![0u64; n / 64 + 1];
            for &(_, i) in best.values() {
                want[i / 64] |= 1 << (i % 64);
            }
            let colliding = |h: u64| ((h & 3) as u32) | 1; // only 2 distinct fingerprints
            let fp = if round % 2 == 0 { super::fingerprint as fn(u64) -> u32 } else { colliding };
            assert_eq!(dedup_bucket(&pieces, fp), want, "round {round}");
        }
    }

    #[test]
    fn record_sizes() {
        // ADR-015: keep the per-line record small; it is written once per input line.
        assert_eq!(std::mem::size_of::<super::RawEv>(), 44);
        assert_eq!(std::mem::size_of::<super::Ev>(), 24);
    }
}

/// Development benchmark (`SS_STAGE=parsebench`): time the events parse loop with
/// one more piece of work per variant, to see what each piece costs (ADR-017, ADR-034).
pub fn parse_bench(src: &crate::input::Source, ctx: &Ctx) {
    use std::hint::black_box;
    let bytes: u64 = src.par_map(|c| c.len() as u64).iter().sum();
    let variants: [(&str, u8); 7] = [
        ("1 scan lines + fields (for_each_line)", 1),
        ("2 + ts", 2),
        ("3 + ingest, price, type", 3),
        ("4 + event_id wide key", 4),
        ("5 + user_id pack (interns long IDs)", 5),
        ("6 + product lookup", 6),
        ("7 full parse_chunk (+ bucket push)", 7),
    ];
    for round in 0..3 {
        for &(name, v) in &variants {
            let t = std::time::Instant::now();
            let sums: Vec<u64> = src.par_map(|chunk| {
                if v == 7 {
                    let b = parse_chunk(chunk, ctx);
                    return b.iter().map(|x| x.len() as u64).sum();
                }
                let mut acc = 0u64;
                csv::for_each_line::<7>(chunk, |f| {
                    acc = acc.wrapping_add(f[0].map_or(0, |x| x.len()) as u64);
                    if v < 2 {
                        return;
                    }
                    let ts = f[2].and_then(ts::parse_ts).unwrap_or(BAD_TS);
                    acc = acc.wrapping_add(ts as u64);
                    if v < 3 {
                        return;
                    }
                    let etype = event_type(f[4]);
                    let ingest = f[3].map_or(i64::MAX, ts::parse_ingest);
                    let price = if etype == PURCHASE { parse_price(f[6]) } else { 0 };
                    acc = acc.wrapping_add(ingest as u64 ^ price as u64 ^ etype as u64);
                    if v < 4 {
                        return;
                    }
                    let eid = f[0].map_or(ids::Wide::default(), ids::wide);
                    acc = acc.wrapping_add(eid.lo ^ eid.hi);
                    if v < 5 {
                        return;
                    }
                    let user = f[1].map_or(0, ids::pack);
                    acc = acc.wrapping_add(user);
                    if v < 6 {
                        return;
                    }
                    let cat = f[5].map_or(NO_PRODUCT, |p| ctx.category_of(p));
                    acc = acc.wrapping_add(cat as u64);
                });
                black_box(acc)
            });
            black_box(sums);
            let s = t.elapsed().as_secs_f64();
            eprintln!("round {round}  {name:<40} {:6.3}s  {:5.2} GB/s", s, bytes as f64 / s / 1e9);
        }
    }
}
