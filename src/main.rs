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
mod numindex;
mod output;
mod par;
mod partition;
mod table;
mod sessions;
mod ts;

use table::FlatMap;
use output::Col;
use parquet::data_type::ByteArray;
use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// mimalloc keeps freed memory for reuse instead of returning it to the OS,
/// which cuts page faults from the many per-partition buffers (ADR-020).
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Insert only if the key is new; true if it was inserted (first row wins).
trait InsertNew<K, V> {
    fn try_insert_new(&mut self, k: K, v: V) -> bool;
}

impl<K: Hash + Eq, V, S: std::hash::BuildHasher> InsertNew<K, V> for HashMap<K, V, S> {
    fn try_insert_new(&mut self, k: K, v: V) -> bool {
        if self.contains_key(&k) {
            return false;
        }
        self.insert(k, v);
        true
    }
}

/// Read-only lookups shared by every stage.
pub struct Ctx {
    pub window_start: i64,
    pub window_end: i64,
    products: FlatMap<u16>,                       // base-37 encodable IDs
    products_other: HashMap<Vec<u8>, u16>,        // any other ID, by bytes
    products_num: numindex::NumIndex,             // "prefix + number" IDs, direct index
    unknown_cat: u16,
    categories: Vec<Vec<u8>>, // id -> name, ids in byte-wise name order
    campaigns: FlatMap<u16>,
    campaigns_other: HashMap<Vec<u8>, u16>,
    campaigns_num: numindex::NumIndex,
    campaign_names: Vec<Vec<u8>>,
    channels: Vec<Vec<u8>>,
}

impl Ctx {
    #[inline]
    /// Category of a product ID, `unknown` if not in products.csv. No interning:
    /// an ID that is not base-37 encodable is looked up by its bytes.
    pub fn category_of(&self, product: &[u8]) -> u16 {
        // Direct index for the catalog's own "prefix + number" shape (ADR-032).
        if let numindex::Lookup::Shaped(v) = self.products_num.get(product) {
            return v.unwrap_or(self.unknown_cat);
        }
        match ids::try_encode(product) {
            Some(k) => self.products.get(k),
            None => self.products_other.get(product).copied(),
        }
        .unwrap_or(self.unknown_cat)
    }
    #[inline]
    pub fn campaign_of(&self, campaign: &[u8]) -> Option<u16> {
        if let numindex::Lookup::Shaped(v) = self.campaigns_num.get(campaign) {
            return v;
        }
        match ids::try_encode(campaign) {
            Some(k) => self.campaigns.get(k),
            None => self.campaigns_other.get(campaign).copied(),
        }
    }
}

fn two_col_rows(path: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    let data = input::read_small(path);
    let mut out = Vec::new();
    csv::for_each_line::<2>(input::body(&data), |f| {
        if let (Some(a), Some(b)) = (f[0], f[1]) {
            out.push((a.to_vec(), b.to_vec()));
        }
    });
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
    // "prefix + number" IDs go to a direct index; the rest to the exact general maps.
    let (products_num, prest) = numindex::NumIndex::build(prows.iter().map(|(p, c)| (p.as_slice(), cat_id(c))));
    let mut products = FlatMap::with_capacity(prest.len());
    let mut products_other = HashMap::new();
    for (p, cat) in prest {
        let c = cat;
        match ids::try_encode(p) {
            Some(k) => {
                products.insert_new(k, c);
            }
            None => {
                products_other.entry(p.to_vec()).or_insert(c);
            }
        }
    }
    let unknown_cat = cat_id(b"unknown");

    // Campaign ids in first-seen order (first row wins), then the same split as products.
    let crows = two_col_rows(&data.join("campaigns.csv"));
    let mut seen: HashMap<Vec<u8>, u16> = HashMap::new();
    let mut campaign_names = Vec::new();
    let mut channels = Vec::new();
    for (c, ch) in crows {
        if seen.try_insert_new(c.clone(), campaign_names.len() as u16) {
            campaign_names.push(c);
            channels.push(ch);
        }
    }
    let (campaigns_num, crest) =
        numindex::NumIndex::build(campaign_names.iter().enumerate().map(|(i, c)| (c.as_slice(), i as u16)));
    let mut campaigns = FlatMap::with_capacity(crest.len());
    let mut campaigns_other: HashMap<Vec<u8>, u16> = HashMap::new();
    for (c, id) in crest {
        match ids::try_encode(c) {
            Some(k) => {
                campaigns.insert_new(k, id);
            }
            None => {
                campaigns_other.insert(c.to_vec(), id);
            }
        }
    }
    // Ev::ts is a u32 offset from window_start (ADR-023).
    assert!(window_end - window_start <= u32::MAX as i64, "window longer than 49 days");
    Ctx {
        window_start,
        window_end,
        products,
        products_other,
        products_num,
        unknown_cat,
        categories: names,
        campaigns,
        campaigns_other,
        campaigns_num,
        campaign_names,
        channels,
    }
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

    if std::env::var("SS_STAGE").as_deref() == Ok("parsebench") {
        events::parse_bench(&ev_src, &ctx);
        return;
    }
    // Links run while events are parsed: parsing needs no person mapping (ADR-026).
    let (persons, parsed) = rayon::join(|| links::persons(&ln_src), || events::parse(&ev_src, &ctx));
    lap("links + events parse");

    let (ev, ck) = rayon::join(|| events::dedup_route(parsed, &ctx, &persons), || clicks::load(&ck_src, &ctx, &persons));
    lap("events dedup+route + clicks");

    let first_day = ctx.window_start.div_euclid(ts::MS_PER_DAY);
    let ndays = ((ctx.window_end - 1).div_euclid(ts::MS_PER_DAY) - first_day + 1) as usize;
    let ncamp = ctx.campaign_names.len();
    let totals = partition::run(&ev, &ck, ctx.categories.len(), ncamp, first_day, ndays, ctx.window_start);
    // Don't free the routed events and clicks: the OS reclaims all memory at exit
    // in one step, which is much faster than freeing it piece by piece (ADR-020).
    std::mem::forget((ev, ck));
    let cd = totals.cd;
    let n_sessions = totals.sessions;
    let user_days = par::par_concat(totals.days);
    lap("partitions: sort, sessions, attribution");

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

    lap("output user_daily");
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
    lap("output campaign_daily");
    eprintln!("[{:>8.3}s] TOTAL ({} sessions, {} user-days)", t0.elapsed().as_secs_f64(), n_sessions, user_days.len());
    // Exit without running destructors: the output files are written and closed,
    // and the scorer's clock runs until the process exits (ADR-020).
    std::process::exit(0);
}
