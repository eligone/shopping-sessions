//! Credit and assists (rules.md, "Clicks" steps 4-7) -> campaign_daily counters.
//!
//! Sessions and clicks are both sorted by person (same key order), so each
//! person's sessions and clicks are walked together.
//! - Credit: latest click with ts <= start (ties: smallest click_id, which is
//!   first in its ts group), credited if start - ts <= 7 days, else organic.
//! - Assists: per (person, campaign), click times merge into disjoint ranges
//!   [first, last + 7d]; each range covers a contiguous run of that person's
//!   sessions, counted per day. Linear in clicks, even for shared devices.

use crate::clicks::Click;
use crate::sessions::Session;
use crate::ts::MS_PER_DAY;

const WEEK: i64 = 7 * MS_PER_DAY;

/// Counters indexed by [campaign][day]; campaign `ncamp` is organic.
pub struct CampaignDaily {
    pub ndays: usize,
    pub first_day: i64,
    pub sessions: Vec<u64>,
    pub purchasing: Vec<u64>,
    pub revenue: Vec<u64>,
    pub assisted: Vec<u64>,
}

impl CampaignDaily {
    pub fn new(ncamp: usize, first_day: i64, ndays: usize) -> Self {
        let n = (ncamp + 1) * ndays;
        CampaignDaily { ndays, first_day, sessions: vec![0; n], purchasing: vec![0; n], revenue: vec![0; n], assisted: vec![0; n] }
    }
    pub fn add(&mut self, o: &CampaignDaily) {
        for (a, b) in [(&mut self.sessions, &o.sessions), (&mut self.purchasing, &o.purchasing), (&mut self.revenue, &o.revenue), (&mut self.assisted, &o.assisted)] {
            for (x, y) in a.iter_mut().zip(b) {
                *x += y;
            }
        }
    }
}

/// Credit and assists for sessions and clicks of the same persons, both sorted by person.
pub fn walk<'c>(sess: &[Session], clicks_of: impl Fn(crate::ids::Key) -> &'c [Click], ncamp: usize, cd: &mut CampaignDaily) {
    let nd = cd.ndays;
    let day_idx = |start: i64| (start.div_euclid(MS_PER_DAY) - cd.first_day) as usize;
    let mut i = 0;
    let mut by_camp: Vec<(u16, i64)> = Vec::new();
    let mut day_blocks: Vec<(usize, usize, usize)> = Vec::new(); // (day index, first, last) session offsets
    while i < sess.len() {
        let person = sess[i].person;
        let mut j = i + 1;
        while j < sess.len() && sess[j].person == person {
            j += 1;
        }
        let ps = &sess[i..j];
        // This person's clicks, sorted by (ts, click_id), copies dropped (ADR-042).
        let pc = clicks_of(person);

        // Credit.
        let mut p = 0;
        let mut group = 0;
        for s in ps {
            while p < pc.len() && pc[p].ts <= s.start {
                if p == 0 || pc[p].ts != pc[p - 1].ts {
                    group = p;
                }
                p += 1;
            }
            let camp = if p > 0 && s.start - pc[group].ts <= WEEK { pc[group].camp as usize } else { ncamp };
            let k = camp * nd + day_idx(s.start);
            cd.sessions[k] += 1;
            cd.revenue[k] += s.revenue;
            if s.purchase {
                cd.purchasing[k] += 1;
            }
        }

        // Assists.
        if !pc.is_empty() {
            day_blocks.clear();
            for (o, s) in ps.iter().enumerate() {
                let d = day_idx(s.start);
                match day_blocks.last_mut() {
                    Some(b) if b.0 == d => b.2 = o,
                    _ => day_blocks.push((d, o, o)),
                }
            }
            by_camp.clear();
            by_camp.extend(pc.iter().map(|c| (c.camp, c.ts)));
            by_camp.sort_unstable();
            let mut k = 0;
            while k < by_camp.len() {
                let camp = by_camp[k].0;
                let lo = by_camp[k].1;
                let mut last = lo;
                k += 1;
                while k < by_camp.len() && by_camp[k].0 == camp && by_camp[k].1 - last <= WEEK {
                    last = by_camp[k].1;
                    k += 1;
                }
                let hi = last + WEEK;
                // Sessions with lo <= start <= hi.
                let r1 = ps.partition_point(|s| s.start < lo);
                let r2 = ps.partition_point(|s| s.start <= hi);
                if r1 >= r2 {
                    continue;
                }
                let mut b = day_blocks.partition_point(|b| b.2 < r1);
                while b < day_blocks.len() && day_blocks[b].1 < r2 {
                    let (d, f, l) = day_blocks[b];
                    let n = l.min(r2 - 1) + 1 - f.max(r1);
                    cd.assisted[camp as usize * nd + d] += n as u64;
                    b += 1;
                }
            }
        }
        i = j;
    }
}
