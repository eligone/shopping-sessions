//! Memory-map the input CSVs and cut them into line-aligned chunks for parallel parsing.

use memmap2::Mmap;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Target chunk size. Small enough that the biggest file (about 1/5 of the data)
/// splits across all threads, big enough that per-chunk overhead is negligible.
pub const CHUNK: usize = 4 << 20;

pub struct Files {
    maps: Vec<Mmap>,
}

/// Every regular `*.csv` file directly in `dir` (symlinks to files are followed).
pub fn csv_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "csv") && p.is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

impl Files {
    pub fn open(paths: &[PathBuf]) -> Files {
        let maps = paths
            .iter()
            .filter_map(|p| {
                let f = File::open(p).ok()?;
                if f.metadata().ok()?.len() == 0 {
                    return None;
                }
                let m = unsafe { Mmap::map(&f).ok()? };
                // Start reading the whole file now, in the background (ADR-013), so a
                // cold page cache fills while we parse instead of one fault at a time.
                #[cfg(unix)]
                {
                    let _ = m.advise(memmap2::Advice::WillNeed);
                    let _ = m.advise(memmap2::Advice::Sequential);
                }
                Some(m)
            })
            .collect();
        Files { maps }
    }

    /// Line-aligned chunks of every file's body (the header line removed).
    pub fn chunks(&self) -> Vec<&[u8]> {
        let mut out = Vec::new();
        for m in &self.maps {
            let data: &[u8] = m;
            let body = match data.iter().position(|&c| c == b'\n') {
                Some(p) => &data[p + 1..],
                None => continue, // header only, no newline
            };
            split_chunks(body, CHUNK, &mut out);
        }
        out
    }
}

/// Split `body` into pieces of about `target` bytes, each ending just after a `\n`
/// (except possibly the last piece).
pub fn split_chunks<'a>(body: &'a [u8], target: usize, out: &mut Vec<&'a [u8]>) {
    let mut start = 0;
    while start < body.len() {
        let mut end = (start + target).min(body.len());
        if end < body.len() {
            match body[end..].iter().position(|&c| c == b'\n') {
                Some(p) => end += p + 1,
                None => end = body.len(),
            }
        }
        out.push(&body[start..end]);
        start = end;
    }
}

/// A small whole file (products.csv, campaigns.csv, manifest.json).
pub fn read_small(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// Body of a small CSV with its header line removed.
pub fn body(data: &[u8]) -> &[u8] {
    match data.iter().position(|&c| c == b'\n') {
        Some(p) => &data[p + 1..],
        None => &[],
    }
}
