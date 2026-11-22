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
use crate::table::FlatMap;
use rayon::prelude::*;

pub const PART_BITS: u32 = 9; // 512 partitions (ADR-052)
pub const PARTS: usize = 1 << PART_BITS;

/// Bucket / partition of a key. Taken from the MIDDLE of the hash: hash tables use
/// the low bits to pick a slot and the top 7 bits as a fingerprint, so a bucket
/// chosen from either end would make every key in it look alike to its table.
#[inline]
pub fn bucket(k: Key) -> usize {
    ((ids::mix(k) >> 28) & (PARTS as u64 - 1)) as usize
}

/// Bucket of a wide event_id key (ADR-031): same middle bits of its mix.
#[inline]
pub fn bucket_wide(k: &ids::Wide) -> usize {
    ((k.mix() >> 28) & (PARTS as u64 - 1)) as usize
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
            let (e, c, crange, lid) = group_partition(ev, ck, p);
            let (sess, days) = sessions::scan(&e, ncat, base);
            attribution::walk(
                &sess,
                |person| {
                    let l = lid.get(person).expect("every session's person has a local id") as usize;
                    &c[crange[l] as usize..crange[l + 1] as usize]
                },
                ncamp,
                &mut acc.cd,
            );
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

/// `[producer][partition]` -> `[partition][producer]`, moving the vectors (no copy).
pub fn transpose<T>(v: Vec<Vec<Vec<T>>>) -> Vec<Vec<Vec<T>>> {
    let mut out: Vec<Vec<Vec<T>>> = (0..PARTS).map(|_| Vec::with_capacity(v.len())).collect();
    for row in v {
        for (p, x) in row.into_iter().enumerate() {
            out[p].push(x);
        }
    }
    out
}

/// Group one partition by person with a counting sort (ADR-042), instead of a
/// gather copy plus a comparison sort on (person, ts):
/// 1. give each person a dense local id (first seen order), counting events per id;
/// 2. place every event directly at its final offset (no gather copy);
/// 3. sort each person's run by ts (insertion sort for short runs).
/// Clicks get the same ids; each person's run is sorted by (ts, click_id) and
/// duplicate copies (identical values) are dropped.
/// Returns events grouped by person and sorted by ts within a person, clicks per
/// person (`crange[lid]..crange[lid + 1]`), and the id map.
pub fn group_partition(
    ev: &[Vec<Vec<Ev>>],
    ck: &[Vec<Vec<Click>>],
    p: usize,
) -> (Vec<Ev>, Vec<Click>, Vec<u32>, FlatMap<u32>) {
    let n_ev: usize = ev.iter().map(|t| t[p].len()).sum();
    let n_ck: usize = ck.iter().map(|t| t[p].len()).sum();
    let mut lid: FlatMap<u32> = FlatMap::with_capacity((n_ev / 8).max(64));
    let mut next: u32 = 0;
    let mut id_of = |k: Key, lid: &mut FlatMap<u32>| -> u32 {
        if let Some(v) = lid.get(k) {
            return v;
        }
        lid.insert_new(k, next);
        next += 1;
        next - 1
    };
    let mut ev_l: Vec<u32> = Vec::with_capacity(n_ev);
    for t in ev {
        for x in &t[p] {
            ev_l.push(id_of(x.person, &mut lid));
        }
    }
    let mut ck_l: Vec<u32> = Vec::with_capacity(n_ck);
    for t in ck {
        for x in &t[p] {
            ck_l.push(id_of(x.person, &mut lid));
        }
    }
    let nl = next as usize;

    // Events: counting sort by local id, then by ts within each person.
    let epos = offsets(&ev_l, nl);
    let e = scatter(ev.iter().map(|t| &t[p]), &ev_l, &epos);
    let mut e = e;
    for l in 0..nl {
        let r = &mut e[epos[l] as usize..epos[l + 1] as usize];
        if r.len() > 1 {
            if r.len() <= 32 {
                insertion_sort_by(r, |a, b| a.ts < b.ts);
            } else {
                r.sort_unstable_by_key(|x| x.ts);
            }
        }
    }

    // Clicks: counting sort by local id, sort each run by (ts, click_id), drop copies.
    let cpos = offsets(&ck_l, nl);
    let mut c = scatter(ck.iter().map(|t| &t[p]), &ck_l, &cpos);
    let mut crange: Vec<u32> = Vec::with_capacity(nl + 1);
    let mut w = 0usize;
    crange.push(0);
    for l in 0..nl {
        let (s, e2) = (cpos[l] as usize, cpos[l + 1] as usize);
        let r = &mut c[s..e2];
        r.sort_unstable_by(|a, b| a.ts.cmp(&b.ts).then_with(|| ids::cmp_ids(a.cid, b.cid)));
        for k in s..e2 {
            let x = c[k];
            if w > crange[l] as usize && c[w - 1].cid == x.cid && c[w - 1].ts == x.ts {
                continue; // a duplicate copy of the previous click
            }
            c[w] = x;
            w += 1;
        }
        crange.push(w as u32);
    }
    c.truncate(w);
    (e, c, crange, lid)
}

/// Start offset of each local id's run (length nl + 1).
fn offsets(ids: &[u32], nl: usize) -> Vec<u32> {
    let mut pos = vec![0u32; nl + 1];
    for &l in ids {
        pos[l as usize + 1] += 1;
    }
    for i in 0..nl {
        pos[i + 1] += pos[i];
    }
    pos
}

/// Write the pieces' items, in order, to their runs' next free slots.
fn scatter<'a, T: Copy + 'a>(pieces: impl Iterator<Item = &'a Vec<T>>, ids: &[u32], pos: &[u32]) -> Vec<T> {
    let n = ids.len();
    let mut out: Vec<T> = Vec::with_capacity(n);
    let mut cur: Vec<u32> = pos[..pos.len() - 1].to_vec();
    {
        let slots = &mut out.spare_capacity_mut()[..n];
        let mut k = 0usize;
        for piece in pieces {
            for x in piece {
                let l = ids[k] as usize;
                slots[cur[l] as usize].write(*x);
                cur[l] += 1;
                k += 1;
            }
        }
    }
    // SAFETY: the runs' sizes are the counts of ids, so every slot below n was written once.
    unsafe { out.set_len(n) };
    out
}

#[inline]
fn insertion_sort_by<T: Copy>(r: &mut [T], less: impl Fn(&T, &T) -> bool) {
    for i in 1..r.len() {
        let x = r[i];
        let mut j = i;
        while j > 0 && less(&x, &r[j - 1]) {
            r[j] = r[j - 1];
            j -= 1;
        }
        r[j] = x;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counting-sort grouping yields, per person, exactly what the old
    /// gather + comparison sort + dedup did (ADR-042).
    #[test]
    fn fuzz_group_partition_matches_sort() {
        let mut seed: u64 = 0x7A3F_19C4_D2B8_5E61;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..40 {
            let persons: Vec<Key> = (0..(1 + next() % 300)).map(|i| 1 + i * 7919 + (next() % 3)).collect();
            let producers = 1 + (next() % 6) as usize;
            let mut ev: Vec<Vec<Vec<Ev>>> = vec![vec![Vec::new()]; producers];
            let mut ck: Vec<Vec<Vec<Click>>> = vec![vec![Vec::new()]; producers];
            for _ in 0..(next() % 3000) {
                let person = persons[(next() % persons.len() as u64) as usize];
                let e = Ev { person, ts: (next() % 200) as u32, price: (next() % 5) as u32, cat: (next() % 3) as u16, etype: 1 + (next() % 3) as u8 };
                ev[(next() % producers as u64) as usize][0].push(e);
            }
            for _ in 0..(next() % 1500) {
                // Copies of a click are identical (rules.md), so every field follows from its click_id;
                // some persons appear only in clicks.
                let cid = 1 + next() % 400;
                let person = persons[(cid % persons.len() as u64) as usize] + if cid % 10 == 0 { 100_000 } else { 0 };
                let c = Click { person, cid, ts: ((cid * 37) % 200) as i64, camp: (cid % 5) as u16 };
                let dup = next() % 5 == 0;
                ck[(next() % producers as u64) as usize][0].push(c);
                if dup {
                    ck[(next() % producers as u64) as usize][0].push(c); // identical copy
                }
            }
            let (e, c, crange, lid) = group_partition(&ev, &ck, 0);
            // Old way, as the reference.
            let mut re: Vec<Ev> = ev.iter().flat_map(|t| t[0].iter().copied()).collect();
            re.sort_unstable_by(|a, b| a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)));
            let mut rc: Vec<Click> = ck.iter().flat_map(|t| t[0].iter().copied()).collect();
            rc.sort_unstable_by(|a, b| a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)).then_with(|| ids::cmp_ids(a.cid, b.cid)));
            rc.dedup_by(|b, a| a.cid == b.cid && a.person == b.person && a.ts == b.ts);
            // Per person: same multiset of events in ts order (ties in any order), same clicks.
            let key = |x: &Ev| (x.ts, x.price, x.cat, x.etype);
            let mut people: Vec<Key> = re.iter().map(|x| x.person).chain(rc.iter().map(|x| x.person)).collect();
            people.sort_unstable();
            people.dedup();
            for &person in &people {
                let mut got: Vec<_> = e.iter().filter(|x| x.person == person).map(key).collect();
                assert!(got.windows(2).all(|w| w[0].0 <= w[1].0), "events not in ts order");
                let mut want: Vec<_> = re.iter().filter(|x| x.person == person).map(key).collect();
                got.sort_unstable();
                want.sort_unstable();
                assert_eq!(got, want);
                let l = lid.get(person).expect("person has an id") as usize;
                let gc: Vec<(i64, u64, u16)> = c[crange[l] as usize..crange[l + 1] as usize].iter().map(|x| (x.ts, x.cid, x.camp)).collect();
                let wc: Vec<(i64, u64, u16)> = rc.iter().filter(|x| x.person == person).map(|x| (x.ts, x.cid, x.camp)).collect();
                assert_eq!(gc, wc);
            }
            // Each person's events form one contiguous run.
            let mut seen = std::collections::HashSet::new();
            let mut prev = None;
            for x in &e {
                if prev != Some(x.person) {
                    assert!(seen.insert(x.person), "person split into two runs");
                    prev = Some(x.person);
                }
            }
        }
    }
}
