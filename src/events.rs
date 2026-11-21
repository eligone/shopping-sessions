//! Events: parse -> dedup on event_id (smallest ingest_ts, BEFORE validation)
//! -> validate -> drop late -> compact records keyed by person.

use crate::csv;
use crate::ids::{self, Key, KeyMap};
use crate::ts;
use crate::Ctx;
use rayon::prelude::*;

pub const VIEW: u8 = 1;
pub const CART: u8 = 2;
pub const PURCHASE: u8 = 3;

const BUCKET_BITS: u32 = 6;
const BUCKETS: usize = 1 << BUCKET_BITS;
const BAD_TS: i64 = i64::MIN;
const NO_PRODUCT: u16 = u16::MAX;
const MAX_PRICE: u32 = 10_000_000;

/// One parsed (not yet validated) copy of an event.
#[derive(Clone, Copy)]
struct RawEv {
    eid: Key,
    user: Key, // 0 = null (a packed non-empty ID is never 0)
    ts: i64,   // BAD_TS = bad timestamp
    ingest: i64,
    price: u32, // 0 = not a valid purchase price
    cat: u16,   // NO_PRODUCT = null product_id
    etype: u8,  // 0 = not view / cart / purchase
}

/// A surviving event.
#[derive(Clone, Copy)]
pub struct Ev {
    pub person: Key,
    pub ts: i64,
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
    for line in csv::lines(chunk) {
        let mut f = [None; 7];
        if !csv::fields::<7>(line, &mut f) {
            continue;
        }
        let Some(eid) = f[0] else { continue };
        let eid = ids::pack(eid);
        let etype = event_type(f[4]);
        let r = RawEv {
            eid,
            user: f[1].map_or(0, ids::pack),
            ts: f[2].and_then(ts::parse_ts).unwrap_or(BAD_TS),
            ingest: f[3].map_or(i64::MAX, ts::parse_ingest),
            price: if etype == PURCHASE { parse_price(f[6]) } else { 0 },
            cat: match f[5] {
                None => NO_PRODUCT,
                Some(p) => ctx.category_of(ids::pack(p)),
            },
            etype,
        };
        b[(ids::mix(eid) >> (64 - BUCKET_BITS)) as usize].push(r);
    }
    b
}

/// Parse, dedup, validate. Returns surviving events in arbitrary order.
pub fn load(src: &crate::input::Source, ctx: &Ctx, persons: &KeyMap<Key>) -> Vec<Ev> {
    let t0 = std::time::Instant::now();
    let parsed: Vec<Vec<Vec<RawEv>>> = src.par_map(|c| parse_chunk(c, ctx));
    eprintln!("    events parse   {:.3}s", t0.elapsed().as_secs_f64()); let t1 = std::time::Instant::now();
    let (ws, we) = (ctx.window_start, ctx.window_end);

    let per_bucket: Vec<Vec<Ev>> = (0..BUCKETS)
        .into_par_iter()
        .map(|bi| {
            let n: usize = parsed.iter().map(|p| p[bi].len()).sum();
            // Dedup: keep the copy with the smallest ingest_ts. Equal ingest_ts copies are identical.
            let mut best: KeyMap<u32> = ids::key_map(n);
            let mut kept: Vec<RawEv> = Vec::with_capacity(n);
            for p in &parsed {
                for r in &p[bi] {
                    match best.get(&r.eid) {
                        Some(&i) => {
                            if r.ingest < kept[i as usize].ingest {
                                kept[i as usize] = *r;
                            }
                        }
                        None => {
                            best.insert(r.eid, kept.len() as u32);
                            kept.push(*r);
                        }
                    }
                }
            }
            // Validate the kept copy, then drop late events.
            kept.iter()
                .filter(|r| {
                    r.user != 0
                        && r.ts != BAD_TS
                        && r.ts >= ws
                        && r.ts < we
                        && r.etype != 0
                        && r.cat != NO_PRODUCT
                        && (r.etype != PURCHASE || r.price != 0)
                        && r.ingest.saturating_sub(r.ts) <= ts::MS_PER_DAY
                })
                .map(|r| Ev {
                    person: persons.get(&r.user).copied().unwrap_or(r.user),
                    ts: r.ts,
                    price: r.price,
                    cat: r.cat,
                    etype: r.etype,
                })
                .collect()
        })
        .collect();

    eprintln!("    events dedup   {:.3}s", t1.elapsed().as_secs_f64()); let t2 = std::time::Instant::now();
    let total: usize = per_bucket.iter().map(|v| v.len()).sum();
    let mut out = Vec::with_capacity(total);
    for v in per_bucket {
        out.extend_from_slice(&v);
    }
    eprintln!("    events concat  {:.3}s", t2.elapsed().as_secs_f64());
    out
}
