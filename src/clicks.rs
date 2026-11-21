//! Clicks: parse -> validate -> dedup on click_id -> sorted by (person, ts, click_id).
//!
//! Copies of a click have identical values, so validating before dedup gives
//! the same result as the rules' order, and after sorting the copies are adjacent.

use crate::csv;
use crate::ids::{self, Key, KeyMap};
use crate::ts;
use crate::Ctx;
use rayon::prelude::*;

#[derive(Clone, Copy)]
pub struct Click {
    pub person: Key,
    pub cid: Key,
    pub ts: i64,
    pub camp: u16,
}

pub fn load(src: &crate::input::Source, ctx: &Ctx, persons: &KeyMap<Key>) -> Vec<Click> {
    let lo = ctx.window_start - 7 * ts::MS_PER_DAY;
    let hi = ctx.window_end;
    let parts: Vec<Vec<Click>> = src.par_map(|c| {
            let mut out = Vec::with_capacity(c.len() / 45 + 16);
            for line in csv::lines(c) {
                let mut f = [None; 4];
                if !csv::fields::<4>(line, &mut f) {
                    continue;
                }
                let (Some(cid), Some(user), Some(camp), Some(t)) = (f[0], f[1], f[2], f[3]) else {
                    continue;
                };
                let Some(camp) = ctx.campaign_of(ids::pack(camp)) else { continue };
                let Some(t) = ts::parse_ts(t) else { continue };
                if t < lo || t >= hi {
                    continue;
                }
                let user = ids::pack(user);
                out.push(Click {
                    person: persons.get(&user).copied().unwrap_or(user),
                    cid: ids::pack(cid),
                    ts: t,
                    camp,
                });
            }
            out
        });
    let mut v: Vec<Click> = Vec::with_capacity(parts.iter().map(|p| p.len()).sum());
    for p in parts {
        v.extend_from_slice(&p);
    }
    v.par_sort_unstable_by(|a, b| {
        a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)).then_with(|| ids::cmp_ids(a.cid, b.cid))
    });
    v.dedup_by(|b, a| a.cid == b.cid && a.person == b.person && a.ts == b.ts);
    v
}
