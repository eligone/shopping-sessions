//! Shopping Sessions pipeline (Rust). See rules.md and docs/adr/.
//!
//! Reads  $DATA_DIR    (default /data)
//! Writes $OUTPUT_DIR  (default /output): user_daily/ and campaign_daily/ as Parquet
//! Logs per-stage timings to stderr.

mod attribution;
mod clicks;
mod csv;
mod events;
mod ids;
mod input;
mod links;
mod output;
mod sessions;
mod ts;

use ids::{Key, KeyMap};
use output::Col;
use parquet::data_type::ByteArray;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Read-only lookups shared by every stage.
pub struct Ctx {
    pub window_start: i64,
    pub window_end: i64,
    products: KeyMap<u16>,
    unknown_cat: u16,
    categories: Vec<Vec<u8>>, // id -> name, ids in byte-wise name order
    campaigns: KeyMap<u16>,
    campaign_names: Vec<Vec<u8>>,
    channels: Vec<Vec<u8>>,
}

impl Ctx {
    #[inline]
    pub fn category_of(&self, product: Key) -> u16 {
        self.products.get(&product).copied().unwrap_or(self.unknown_cat)
    }
    #[inline]
    pub fn campaign_of(&self, campaign: Key) -> Option<u16> {
        self.campaigns.get(&campaign).copied()
    }
}

fn two_col_rows(path: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    let data = input::read_small(path);
    let mut out = Vec::new();
    for line in csv::lines(input::body(&data)) {
        let mut f = [None; 2];
        if csv::fields::<2>(line, &mut f) {
            if let (Some(a), Some(b)) = (f[0], f[1]) {
                out.push((a.to_vec(), b.to_vec()));
            }
        }
    }
    out
}

/// Pull a string value out of the tiny manifest without a JSON dependency.
fn manifest_value(json: &str, key: &str) -> String {
    let at = json.find(&format!("\"{key}\"")).expect("manifest key") + key.len() + 2;
    let rest = &json[at..];
    let open = rest.find('"').expect("manifest value") + 1;
    let close = rest[open..].find('"').expect("manifest value end") + open;
    rest[open..close].to_string()
}

fn build_ctx(data: &Path) -> Ctx {
    let manifest = String::from_utf8(input::read_small(&data.join("manifest.json"))).expect("manifest utf-8");
    let window_start = ts::parse_f1(&manifest_value(&manifest, "window_start"));
    let window_end = ts::parse_f1(&manifest_value(&manifest, "window_end"));

    let prows = two_col_rows(&data.join("products.csv"));
    let mut names: Vec<Vec<u8>> = prows.iter().map(|r| r.1.clone()).collect();
    names.push(b"unknown".to_vec());
    names.sort();
    names.dedup();
    let cat_id = |n: &[u8]| names.binary_search_by(|x| x.as_slice().cmp(n)).unwrap() as u16;
    let mut products = ids::key_map(prows.len());
    for (p, c) in &prows {
        products.entry(ids::pack(p)).or_insert(cat_id(c));
    }
    let unknown_cat = cat_id(b"unknown");

    let crows = two_col_rows(&data.join("campaigns.csv"));
    let mut campaigns = ids::key_map(crows.len());
    let mut campaign_names = Vec::new();
    let mut channels = Vec::new();
    for (c, ch) in crows {
        let k = ids::pack(&c);
        if campaigns.contains_key(&k) {
            continue;
        }
        campaigns.insert(k, campaign_names.len() as u16);
        campaign_names.push(c);
        channels.push(ch);
    }
    Ctx { window_start, window_end, products, unknown_cat, categories: names, campaigns, campaign_names, channels }
}

fn main() {
    let t0 = Instant::now();
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let data = PathBuf::from(env("DATA_DIR", "/data"));
    let out = PathBuf::from(env("OUTPUT_DIR", "/output"));
    let threads: usize = env("NCPU", "0").parse().unwrap_or(0);
    let threads = if threads == 0 { std::thread::available_parallelism().map_or(8, |n| n.get()) } else { threads };
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().unwrap();

    let mut last = Instant::now();
    let mut lap = |name: &str| {
        eprintln!("[{:>8.3}s] {name}", last.elapsed().as_secs_f64());
        last = Instant::now();
    };

    let ctx = build_ctx(&data);
    lap("manifest, products, campaigns");

    let mode = input::Mode::from_env();
    let (ev_src, ck_src, ln_src) = (
        input::Source::open(&input::csv_files(&data.join("events")), mode),
        input::Source::open(&input::csv_files(&data.join("clicks")), mode),
        input::Source::open(&input::csv_files(&data.join("links")), mode),
    );
    lap(if mode == input::Mode::Mmap { "open (mmap)" } else { "open (pread)" });

    let persons = links::persons(&ln_src);
    lap("links");

    let (ev, ck) = rayon::join(|| events::load(&ev_src, &ctx, &persons), || clicks::load(&ck_src, &ctx, &persons));
    lap("events + clicks");

    let (sessions, user_days) = sessions::build(ev, ctx.categories.len(), threads);
    lap("sessions");

    let first_day = ctx.window_start.div_euclid(ts::MS_PER_DAY);
    let ndays = ((ctx.window_end - 1).div_euclid(ts::MS_PER_DAY) - first_day + 1) as usize;
    let ncamp = ctx.campaign_names.len();
    let cd = attribution::compute(&sessions, &ck, ncamp, first_day, ndays, threads);
    lap("attribution");

    let ud_dir = out.join("user_daily");
    let cd_dir = out.join("campaign_daily");
    std::fs::create_dir_all(&ud_dir).unwrap();
    std::fs::create_dir_all(&cd_dir).unwrap();

    // user_daily: one file per thread-sized slice, written in parallel.
    let parts = threads.max(1);
    let per = user_days.len().div_ceil(parts).max(1);
    let slices: Vec<&[sessions::UserDay]> = if user_days.is_empty() { vec![&user_days[..]] } else { user_days.chunks(per).collect() };
    slices.par_iter().enumerate().for_each(|(i, rows)| {
        let cols = vec![
            Col::Str(rows.iter().map(|r| ByteArray::from(ids::unpack(r.person))).collect()),
            Col::Date(rows.iter().map(|r| r.day).collect()),
            Col::I64(rows.iter().map(|r| r.sessions as i64).collect()),
            Col::I64(rows.iter().map(|r| r.events as i64).collect()),
            Col::I64(rows.iter().map(|r| r.revenue as i64).collect()),
            Col::OptStr(rows.iter().map(|r| r.top.map(|c| ByteArray::from(ctx.categories[c as usize].clone()))).collect()),
        ];
        output::write(&ud_dir.join(format!("part-{i}.parquet")), output::USER_DAILY, cols).expect("write user_daily");
    });

    // campaign_daily
    let (mut cid, mut ch, mut day, mut s, mut p, mut r, mut a) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for c in 0..=ncamp {
        for d in 0..ndays {
            let k = c * ndays + d;
            if cd.sessions[k] == 0 && cd.assisted[k] == 0 {
                continue;
            }
            let (name, chan) = if c == ncamp {
                (b"organic".to_vec(), b"organic".to_vec())
            } else {
                (ctx.campaign_names[c].clone(), ctx.channels[c].clone())
            };
            cid.push(ByteArray::from(name));
            ch.push(ByteArray::from(chan));
            day.push((first_day + d as i64) as i32);
            s.push(cd.sessions[k] as i64);
            p.push(cd.purchasing[k] as i64);
            r.push(cd.revenue[k] as i64);
            a.push(if c == ncamp { 0 } else { cd.assisted[k] as i64 });
        }
    }
    let cols = vec![Col::Str(cid), Col::Str(ch), Col::Date(day), Col::I64(s), Col::I64(p), Col::I64(r), Col::I64(a)];
    output::write(&cd_dir.join("part-0.parquet"), output::CAMPAIGN_DAILY, cols).expect("write campaign_daily");
    lap("output");
    eprintln!("[{:>8.3}s] TOTAL ({} sessions, {} user-days)", t0.elapsed().as_secs_f64(), sessions.len(), user_days.len());
}
