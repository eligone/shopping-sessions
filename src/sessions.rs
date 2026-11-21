//! Sessions and user_daily (rules.md, "Events" steps 5-7).
//! Events are sorted by (person, ts); one linear scan per person finds sessions
//! (gap > 30 min; equal timestamps have gap 0 and always share) and sums each
//! person-day.

use crate::events::{Ev, PURCHASE};
use crate::ids::Key;
use crate::ts::MS_PER_DAY;
use rayon::prelude::*;

const GAP: i64 = 30 * 60 * 1000;

#[derive(Clone, Copy)]
pub struct Session {
    pub person: Key,
    pub start: i64,
    pub revenue: u64,
    pub purchase: bool,
}

pub struct UserDay {
    pub person: Key,
    pub day: i32,
    pub sessions: u64,
    pub events: u64,
    pub revenue: u64,
    pub top: Option<u16>,
}

/// Split `v` (sorted by person) into about `parts` ranges, never splitting a person.
pub fn person_ranges<T>(v: &[T], parts: usize, person: impl Fn(&T) -> Key) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let step = v.len() / parts.max(1) + 1;
    let mut start = 0;
    while start < v.len() {
        let mut end = (start + step).min(v.len());
        while end < v.len() && person(&v[end]) == person(&v[end - 1]) {
            end += 1;
        }
        out.push((start, end));
        start = end;
    }
    out
}

pub fn build(mut ev: Vec<Ev>, ncat: usize, threads: usize) -> (Vec<Session>, Vec<UserDay>) {
    ev.par_sort_unstable_by(|a, b| a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)));
    let ranges = person_ranges(&ev, threads * 16, |e| e.person);
    let parts: Vec<(Vec<Session>, Vec<UserDay>)> = ranges
        .par_iter()
        .map(|&(s, e)| scan(&ev[s..e], ncat))
        .collect();
    let mut sessions = Vec::with_capacity(parts.iter().map(|p| p.0.len()).sum());
    let mut days = Vec::with_capacity(parts.iter().map(|p| p.1.len()).sum());
    for (s, d) in parts {
        sessions.extend(s);
        days.extend(d);
    }
    (sessions, days)
}

struct DayAcc {
    person: Key,
    day: i32,
    sessions: u64,
    events: u64,
    revenue: u64,
    cat_rev: Vec<u64>,
    touched: Vec<u16>,
}

impl DayAcc {
    fn flush(&mut self, out: &mut Vec<UserDay>) {
        if self.sessions == 0 {
            return;
        }
        let mut top: Option<u16> = None;
        let mut best = 0u64;
        for &c in &self.touched {
            let r = self.cat_rev[c as usize];
            if r > best || (r == best && top.is_some_and(|t| c < t)) {
                best = r;
                top = Some(c);
            }
        }
        for &c in &self.touched {
            self.cat_rev[c as usize] = 0;
        }
        self.touched.clear();
        out.push(UserDay {
            person: self.person,
            day: self.day,
            sessions: self.sessions,
            events: self.events,
            revenue: self.revenue,
            top,
        });
        self.sessions = 0;
        self.events = 0;
        self.revenue = 0;
    }
}

fn scan(ev: &[Ev], ncat: usize) -> (Vec<Session>, Vec<UserDay>) {
    let mut sessions = Vec::with_capacity(ev.len() / 4 + 1);
    let mut days = Vec::with_capacity(ev.len() / 6 + 1);
    let mut acc = DayAcc {
        person: 0,
        day: 0,
        sessions: 0,
        events: 0,
        revenue: 0,
        cat_rev: vec![0; ncat],
        touched: Vec::new(),
    };
    let mut i = 0;
    while i < ev.len() {
        // One session: [i, j)
        let person = ev[i].person;
        let start = ev[i].ts;
        let mut j = i + 1;
        while j < ev.len() && ev[j].person == person && ev[j].ts - ev[j - 1].ts <= GAP {
            j += 1;
        }
        let day = start.div_euclid(MS_PER_DAY) as i32;
        if acc.person != person || acc.day != day || acc.sessions == 0 {
            acc.flush(&mut days);
            acc.person = person;
            acc.day = day;
        }
        let mut revenue = 0u64;
        let mut purchase = false;
        for e in &ev[i..j] {
            if e.etype == PURCHASE {
                revenue += e.price as u64;
                purchase = true;
                let c = e.cat as usize;
                if acc.cat_rev[c] == 0 {
                    acc.touched.push(e.cat);
                }
                acc.cat_rev[c] += e.price as u64;
            }
        }
        acc.sessions += 1;
        acc.events += (j - i) as u64;
        acc.revenue += revenue;
        sessions.push(Session { person, start, revenue, purchase });
        i = j;
    }
    acc.flush(&mut days);
    (sessions, days)
}
