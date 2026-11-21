//! Partitioned pipeline (ADR-020).
//!
//! Events are deduplicated in 256 buckets by event_id, and the survivors are routed
//! to 256 partitions by PERSON; clicks are routed to the same person partitions
//! while parsing. Every partition then holds all of its persons' events and
//! clicks, so sorting, sessions, user_daily, credit and assists run per
//! partition: small, cache-sized and independent, with no global sort.

use crate::attribution::{self, CampaignDaily};
use crate::clicks::Click;
use crate::events::Ev;
use crate::ids::{self, Key};
use crate::sessions::{self, UserDay};
use rayon::prelude::*;

pub const PART_BITS: u32 = 8;
pub const PARTS: usize = 1 << PART_BITS;

/// Bucket / partition of a key. Taken from the MIDDLE of the hash: hash tables use
/// the low bits to pick a slot and the top 7 bits as a fingerprint, so a bucket
/// chosen from either end would make every key in it look alike to its table.
#[inline]
pub fn bucket(k: Key) -> usize {
    ((ids::mix(k) >> 28) & (PARTS as u64 - 1)) as usize
}

/// Concatenate one partition's pieces from every producer.
fn gather<'a, T: Copy + 'a>(pieces: impl Iterator<Item = &'a Vec<T>> + Clone) -> Vec<T> {
    let n: usize = pieces.clone().map(|p| p.len()).sum();
    let mut v = Vec::with_capacity(n);
    for p in pieces {
        v.extend_from_slice(p);
    }
    v
}

pub struct Totals {
    pub days: Vec<Vec<UserDay>>,
    pub cd: CampaignDaily,
    pub sessions: usize,
}

/// `ev[producer][partition]`, `ck[producer][partition]`.
pub fn run(
    ev: &[Vec<Vec<Ev>>],
    ck: &[Vec<Vec<Click>>],
    ncat: usize,
    ncamp: usize,
    first_day: i64,
    ndays: usize,
    base: i64,
) -> Totals {
    let empty = || Totals { days: Vec::new(), cd: CampaignDaily::new(ncamp, first_day, ndays), sessions: 0 };
    (0..PARTS)
        .into_par_iter()
        .fold(empty, |mut acc, p| {
            let mut e = gather(ev.iter().map(|t| &t[p]));
            e.sort_unstable_by(|a, b| a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)));
            let mut c = gather(ck.iter().map(|t| &t[p]));
            c.sort_unstable_by(|a, b| {
                a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)).then_with(|| ids::cmp_ids(a.cid, b.cid))
            });
            c.dedup_by(|b, a| a.cid == b.cid && a.person == b.person && a.ts == b.ts);
            let (sess, days) = sessions::scan(&e, ncat, base);
            attribution::walk(&sess, &c, ncamp, &mut acc.cd);
            acc.sessions += sess.len();
            acc.days.push(days);
            acc
        })
        .reduce(empty, |mut a, b| {
            a.cd.add(&b.cd);
            a.days.extend(b.days);
            a.sessions += b.sessions;
            a
        })
}
