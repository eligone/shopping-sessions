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
/// rules validate whichever copy wins dedup (ADR-015). 40 bytes (ADR-023).
#[derive(Clone, Copy)]
struct RawEv {
    eid: Key,
    user: Key, // 0 = null (a non-empty ID's key is never 0)
    ingest: i64, // dedup order: smallest wins
    ts: i64,
    price: u32,  // purchase price (0 for view / cart)
    cat: u16,
    etype: u8,
    valid: bool, // passes every rule check, including "not late"
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

fn parse_chunk(chunk: &[u8], ctx: &Ctx) -> Vec<Vec<RawEv>> {
    let est = chunk.len() / 70 / BUCKETS + 16;
    let mut b: Vec<Vec<RawEv>> = (0..BUCKETS).map(|_| Vec::with_capacity(est)).collect();
    let (ws, we) = (ctx.window_start, ctx.window_end);
    csv::for_each_line::<7>(chunk, |f| {
        let Some(eid) = f[0] else { return };
        let eid = ids::pack(eid);
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
            eid,
            user: f[1].map_or(0, ids::pack),
            ingest,
            ts,
            price,
            cat,
            etype,
            valid,
        };
        b[crate::partition::bucket(eid)].push(r);
    });
    b
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
    let ws = ctx.window_start;

    // Per eid bucket: dedup by location (ADR-015), then route the valid winners.
    let routed: Vec<Vec<Vec<Ev>>> = (0..BUCKETS)
        .into_par_iter()
        .map(|bi| {
            let n: usize = parsed.iter().map(|p| p[bi].len()).sum();
            let mut best: FlatMap<u64> = FlatMap::with_capacity(n);
            for (ci, p) in parsed.iter().enumerate() {
                for (pos, r) in p[bi].iter().enumerate() {
                    let here = (ci as u64) << 32 | pos as u64;
                    best.upsert(r.eid, here, |loc| {
                        let cur = &parsed[(*loc >> 32) as usize][bi][(*loc & 0xFFFF_FFFF) as usize];
                        if r.ingest < cur.ingest {
                            *loc = here;
                        }
                    });
                }
            }
            // Mark each event's winning copy with one bit (ADR-027), indexed by its
            // position in this bucket's records across all chunks, then read the
            // records in their natural order: no random reads, no sort.
            let mut first = Vec::with_capacity(parsed.len() + 1); // first[ci] = index of chunk ci's first record
            let mut acc = 0usize;
            for p in &parsed {
                first.push(acc);
                acc += p[bi].len();
            }
            let mut win = vec![0u64; n / 64 + 1];
            for loc in best.values() {
                let i = first[(loc >> 32) as usize] + (loc & 0xFFFF_FFFF) as usize;
                win[i / 64] |= 1 << (i % 64);
            }
            let est = best.len() / parts * 5 / 4 + 4;
            let mut out: Vec<Vec<Ev>> = (0..parts).map(|_| Vec::with_capacity(est)).collect();
            let mut i = 0usize;
            for p in &parsed {
                for r in &p[bi] {
                    if win[i / 64] >> (i % 64) & 1 == 1 && r.valid {
                        let user = r.user;
                        let person = persons.get(user).unwrap_or(user);
                        out[crate::partition::bucket(person)].push(Ev {
                            person,
                            ts: (r.ts - ws) as u32,
                            price: r.price,
                            cat: r.cat,
                            etype: r.etype,
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
    #[test]
    fn record_sizes() {
        // ADR-015: keep the per-line record small; it is written once per input line.
        assert_eq!(std::mem::size_of::<super::RawEv>(), 40);
        assert_eq!(std::mem::size_of::<super::Ev>(), 24);
    }
}

/// Development benchmark (`SS_STAGE=parsebench`): time the events parse loop with
/// one more piece of work per variant, to see what each piece costs (ADR-017).
pub fn parse_bench(src: &crate::input::Source, ctx: &Ctx) {
    use std::hint::black_box;
    let bytes: u64 = src.par_map(|c| c.len() as u64).iter().sum();
    let variants: [(&str, u8); 6] = [
        ("1 split lines only", 1),
        ("2 + split fields (csv::fields)", 2),
        ("3 + parse ts, ingest, price, type", 3),
        ("4 + pack IDs, product lookup", 4),
        ("5 + mix hash (bucket choice)", 5),
        ("6 full parse_chunk (+ bucket push)", 6),
    ];
    for round in 0..3 {
        for &(name, v) in &variants {
            let t = std::time::Instant::now();
            let sums: Vec<u64> = src.par_map(|chunk| {
                if v == 6 {
                    let b = parse_chunk(chunk, ctx);
                    return b.iter().map(|x| x.len() as u64).sum();
                }
                let mut acc = 0u64;
                for line in csv::lines(chunk) {
                    if v == 1 {
                        acc = acc.wrapping_add(line.len() as u64);
                        continue;
                    }
                    let mut f = [None; 7];
                    if !csv::fields::<7>(line, &mut f) {
                        continue;
                    }
                    if v == 2 {
                        acc = acc.wrapping_add(f[0].map_or(0, |x| x.len()) as u64);
                        continue;
                    }
                    let etype = event_type(f[4]);
                    let ts = f[2].and_then(ts::parse_ts).unwrap_or(BAD_TS);
                    let ingest = f[3].map_or(i64::MAX, ts::parse_ingest);
                    let price = if etype == PURCHASE { parse_price(f[6]) } else { 0 };
                    acc = acc.wrapping_add(ts as u64 ^ ingest as u64 ^ price as u64 ^ etype as u64);
                    if v == 3 {
                        continue;
                    }
                    let eid = f[0].map_or(0, ids::pack);
                    let user = f[1].map_or(0, ids::pack);
                    let cat = f[5].map_or(NO_PRODUCT, |p| ctx.category_of(p));
                    acc = acc.wrapping_add(eid as u64 ^ user as u64 ^ cat as u64);
                    if v == 4 {
                        continue;
                    }
                    acc = acc.wrapping_add(crate::partition::bucket(eid) as u64);
                }
                black_box(acc)
            });
            black_box(sums);
            let s = t.elapsed().as_secs_f64();
            eprintln!("round {round}  {name:<38} {:6.3}s  {:5.2} GB/s", s, bytes as f64 / s / 1e9);
        }
    }
}
