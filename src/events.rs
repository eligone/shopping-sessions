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

/// One parsed copy of an event. Validity is decided here, per copy, because the
/// rules validate whichever copy wins dedup (ADR-015). IDs are stored as two
/// u64 halves so the struct is 8-byte aligned: 56 bytes instead of 64.
#[derive(Clone, Copy)]
struct RawEv {
    eid: [u64; 2],
    user: [u64; 2],
    ingest: i64, // dedup order: smallest wins
    ts: i64,
    price: u32,  // purchase price (0 for view / cart)
    cat: u16,
    etype: u8,
    valid: bool, // passes every rule check, including "not late"
}

#[inline]
fn split(k: Key) -> [u64; 2] {
    [k as u64, (k >> 64) as u64]
}

#[inline]
fn join(k: [u64; 2]) -> Key {
    k[0] as u128 | ((k[1] as u128) << 64)
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
    let (ws, we) = (ctx.window_start, ctx.window_end);
    for line in csv::lines(chunk) {
        let mut f = [None; 7];
        if !csv::fields::<7>(line, &mut f) {
            continue;
        }
        let Some(eid) = f[0] else { continue };
        let eid = ids::pack(eid);
        let etype = event_type(f[4]);
        let ts = f[2].and_then(ts::parse_ts).unwrap_or(BAD_TS);
        let ingest = f[3].map_or(i64::MAX, ts::parse_ingest);
        let price = if etype == PURCHASE { parse_price(f[6]) } else { 0 };
        let cat = match f[5] {
            None => NO_PRODUCT,
            Some(p) => ctx.category_of(ids::pack(p)),
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
            eid: split(eid),
            user: split(f[1].map_or(0, ids::pack)),
            ingest,
            ts,
            price,
            cat,
            etype,
            valid,
        };
        b[(ids::mix(eid) >> (64 - BUCKET_BITS)) as usize].push(r);
    }
    b
}

/// Parse, dedup, validate. Returns surviving events in arbitrary order.
pub fn load(src: &crate::input::Source, ctx: &Ctx, persons: &KeyMap<Key>) -> Vec<Ev> {
    let t0 = std::time::Instant::now();
    let parsed: Vec<Vec<Vec<RawEv>>> = src.par_map(|c| parse_chunk(c, ctx));
    eprintln!("    events parse   {:.3}s", t0.elapsed().as_secs_f64());
    let t1 = std::time::Instant::now();

    // Dedup per bucket without copying records: remember WHERE each event's best
    // copy is, as (chunk << 32 | position), then keep only the valid winners.
    let winners: Vec<Vec<u64>> = (0..BUCKETS)
        .into_par_iter()
        .map(|bi| {
            let n: usize = parsed.iter().map(|p| p[bi].len()).sum();
            let mut best: KeyMap<u64> = ids::key_map(n);
            for (ci, p) in parsed.iter().enumerate() {
                for (pos, r) in p[bi].iter().enumerate() {
                    let here = (ci as u64) << 32 | pos as u64;
                    best.entry(join(r.eid))
                        .and_modify(|loc| {
                            let cur = &parsed[(*loc >> 32) as usize][bi][(*loc & 0xFFFF_FFFF) as usize];
                            if r.ingest < cur.ingest {
                                *loc = here;
                            }
                        })
                        .or_insert(here);
                }
            }
            let mut w: Vec<u64> = best
                .into_values()
                .filter(|&loc| parsed[(loc >> 32) as usize][bi][(loc & 0xFFFF_FFFF) as usize].valid)
                .collect();
            w.sort_unstable(); // read the winners back in memory order
            w
        })
        .collect();
    eprintln!("    events dedup   {:.3}s", t1.elapsed().as_secs_f64());
    let t2 = std::time::Instant::now();

    // One output vector, sized exactly; each bucket fills its own slice in parallel.
    let total: usize = winners.iter().map(|w| w.len()).sum();
    let mut out: Vec<Ev> = Vec::with_capacity(total);
    {
        let spare = &mut out.spare_capacity_mut()[..total];
        let mut slices = Vec::with_capacity(BUCKETS);
        let mut rest = spare;
        for w in &winners {
            let (head, tail) = rest.split_at_mut(w.len());
            slices.push(head);
            rest = tail;
        }
        slices.into_par_iter().zip(winners.par_iter()).enumerate().for_each(|(bi, (slot, w))| {
            for (dst, &loc) in slot.iter_mut().zip(w) {
                let r = &parsed[(loc >> 32) as usize][bi][(loc & 0xFFFF_FFFF) as usize];
                let user = join(r.user);
                dst.write(Ev {
                    person: persons.get(&user).copied().unwrap_or(user),
                    ts: r.ts,
                    price: r.price,
                    cat: r.cat,
                    etype: r.etype,
                });
            }
        });
    }
    // SAFETY: every one of the first `total` slots was written above.
    unsafe { out.set_len(total) };
    eprintln!("    events build   {:.3}s", t2.elapsed().as_secs_f64());
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn record_sizes() {
        // ADR-015: keep the per-line record small; it is written once per input line.
        assert_eq!(std::mem::size_of::<super::RawEv>(), 56);
        assert_eq!(std::mem::size_of::<super::Ev>(), 32);
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
                    let cat = f[5].map_or(NO_PRODUCT, |p| ctx.category_of(ids::pack(p)));
                    acc = acc.wrapping_add(eid as u64 ^ user as u64 ^ cat as u64);
                    if v == 4 {
                        continue;
                    }
                    acc = acc.wrapping_add(ids::mix(eid) >> (64 - BUCKET_BITS));
                }
                black_box(acc)
            });
            black_box(sums);
            let s = t.elapsed().as_secs_f64();
            eprintln!("round {round}  {name:<38} {:6.3}s  {:5.2} GB/s", s, bytes as f64 / s / 1e9);
        }
    }
}
