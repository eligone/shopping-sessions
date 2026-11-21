//! Clicks: parse -> validate -> dedup on click_id -> sorted by (person, ts, click_id).
//!
//! Copies of a click have identical values, so validating before dedup gives
//! the same result as the rules' order, and after sorting the copies are adjacent.

use crate::csv;
use crate::ids::{self, Key, KeyMap};
use crate::ts;
use crate::Ctx;

#[derive(Clone, Copy)]
pub struct Click {
    pub person: Key,
    pub cid: Key,
    pub ts: i64,
    pub camp: u16,
}

/// Parse and validate clicks, routed by person partition: `routed[chunk][partition]`.
/// Copies of a click are identical, so they land in the same partition, where
/// sorting makes them adjacent and dedup drops all but one.
pub fn load(src: &crate::input::Source, ctx: &Ctx, persons: &KeyMap<Key>) -> Vec<Vec<Vec<Click>>> {
    let lo = ctx.window_start - 7 * ts::MS_PER_DAY;
    let hi = ctx.window_end;
    let parts = crate::partition::PARTS;
    src.par_map(|c| {
        let est = c.len() / 45 / parts + 4;
        let mut out: Vec<Vec<Click>> = (0..parts).map(|_| Vec::with_capacity(est)).collect();
        for line in csv::lines(c) {
            let mut f = [None; 4];
            if !csv::fields::<4>(line, &mut f) {
                continue;
            }
            let (Some(cid), Some(user), Some(camp), Some(t)) = (f[0], f[1], f[2], f[3]) else {
                continue;
            };
            let Some(camp) = ctx.campaign_of(camp) else { continue };
            let Some(t) = ts::parse_ts(t) else { continue };
            if t < lo || t >= hi {
                continue;
            }
            let user = ids::pack(user);
            let person = persons.get(&user).copied().unwrap_or(user);
            out[crate::partition::bucket(person)].push(Click { person, cid: ids::pack(cid), ts: t, camp });
        }
        out
    })
}
