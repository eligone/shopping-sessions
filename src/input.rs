//! Input files -> line-aligned chunks, parsed in parallel.
//!
//! Two read modes (ADR-013, ADR-014), chosen by `SS_READ`:
//! - `pread` (default): each task reads its byte range with `pread` into a reused
//!   per-thread buffer, so 8 threads keep 8 large reads in flight and no file page
//!   is ever faulted in through a mapping.
//! - `mmap`: map every file, ask the kernel to read ahead (`WILLNEED`), parse in place.
//!
//! Either way a task owns the lines that START inside its byte range `[off, end)`:
//! it skips the partial line at its front (owned by the previous task) and reads
//! past `end` to finish its last line. Each file's first line is the header.

use memmap2::Mmap;
use rayon::prelude::*;
use std::cell::RefCell;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// Target bytes per task: big enough that per-task overhead is negligible,
/// small enough that the largest file (about 1/5 of the data) spreads over all threads.
pub const CHUNK: u64 = 4 << 20;
/// Read granularity when finishing a task's last line past its range.
const TAIL: usize = 64 << 10;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Pread,
    Mmap,
}

impl Mode {
    pub fn from_env() -> Mode {
        match std::env::var("SS_READ").as_deref() {
            Ok("mmap") => Mode::Mmap,
            _ => Mode::Pread,
        }
    }
}

struct Task {
    file: usize,
    off: u64,
    end: u64,
}

pub struct Source {
    mode: Mode,
    files: Vec<File>,
    sizes: Vec<u64>,
    maps: Vec<Mmap>,
    tasks: Vec<Task>,
}

thread_local! {
    static BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
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

impl Source {
    pub fn open(paths: &[PathBuf], mode: Mode) -> Source {
        let mut files = Vec::new();
        let mut sizes = Vec::new();
        for p in paths {
            let Ok(f) = File::open(p) else { continue };
            let Ok(m) = f.metadata() else { continue };
            if m.len() == 0 {
                continue;
            }
            sizes.push(m.len());
            files.push(f);
        }
        let mut maps = Vec::new();
        if mode == Mode::Mmap {
            for f in &files {
                let m = unsafe { Mmap::map(f).expect("mmap") };
                let _ = m.advise(memmap2::Advice::WillNeed);
                let _ = m.advise(memmap2::Advice::Sequential);
                maps.push(m);
            }
        }
        let mut tasks = Vec::new();
        for (i, &size) in sizes.iter().enumerate() {
            let mut off = 0;
            while off < size {
                let end = (off + CHUNK).min(size);
                tasks.push(Task { file: i, off, end });
                off = end;
            }
        }
        // Biggest files first is irrelevant here: tasks are equal-sized, and rayon steals work.
        Source { mode, files, sizes, maps, tasks }
    }

    /// Run `f` on every task's lines in parallel; results in task order.
    pub fn par_map<R: Send>(&self, f: impl Fn(&[u8]) -> R + Sync + Send) -> Vec<R> {
        self.tasks.par_iter().map(|t| self.with(t, &f)).collect()
    }

    fn with<R>(&self, t: &Task, f: &impl Fn(&[u8]) -> R) -> R {
        match self.mode {
            Mode::Mmap => {
                let data: &[u8] = &self.maps[t.file];
                let (s, e) = owned_range(data, t.off as usize, t.end as usize);
                f(&data[s..e])
            }
            Mode::Pread => BUF.with(|b| {
                let mut buf = b.borrow_mut();
                let (s, e) = self.read_task(t, &mut buf);
                f(&buf[s..e])
            }),
        }
    }

    /// Read `[off-1, end)` plus whatever finishes the last line; return the owned lines' range in `buf`.
    fn read_task(&self, t: &Task, buf: &mut Vec<u8>) -> (usize, usize) {
        let file = &self.files[t.file];
        let size = self.sizes[t.file];
        let begin = t.off.saturating_sub(1); // one byte back: is the byte before `off` a '\n'?
        let n = (t.end - begin) as usize;
        if buf.len() < n {
            buf.resize(n, 0);
        }
        file.read_exact_at(&mut buf[..n], begin).expect("read");
        let mut len = n;
        // Finish the last line: read on until a '\n' or end of file.
        let mut pos = t.end;
        if buf[len - 1] != b'\n' {
            while pos < size {
                let want = TAIL.min((size - pos) as usize);
                if buf.len() < len + want {
                    buf.resize(len + want, 0);
                }
                file.read_exact_at(&mut buf[len..len + want], pos).expect("read");
                if let Some(p) = buf[len..len + want].iter().position(|&c| c == b'\n') {
                    len += p + 1;
                    break;
                }
                len += want;
                pos += want as u64;
            }
        }
        owned_range_from(&buf[..len], (t.end - begin) as usize)
    }
}

/// Owned lines of a task in a fully mapped file: lines starting in `[off, end)`, minus the header.
fn owned_range(data: &[u8], off: usize, end: usize) -> (usize, usize) {
    let start = if off == 0 {
        match data.iter().position(|&c| c == b'\n') {
            Some(p) => p + 1,
            None => return (0, 0),
        }
    } else if data[off - 1] == b'\n' {
        off
    } else {
        match data[off..].iter().position(|&c| c == b'\n') {
            Some(p) => off + p + 1,
            None => return (0, 0),
        }
    };
    if start >= end {
        return (0, 0);
    }
    let stop = if data[end - 1] == b'\n' {
        end
    } else {
        match data[end..].iter().position(|&c| c == b'\n') {
            Some(p) => end + p + 1,
            None => data.len(),
        }
    };
    (start, stop)
}

/// Same rule on a buffer holding file bytes from `begin` (one byte before the task,
/// or 0 for a file's first task) through the end of the task's last line. Skipping
/// through the first '\n' drops the partial line owned by the previous task, or the
/// header for the first task; `end` is the task's end relative to `begin`.
fn owned_range_from(buf: &[u8], end: usize) -> (usize, usize) {
    let start = match buf.iter().position(|&c| c == b'\n') {
        Some(p) => p + 1,
        None => return (0, 0),
    };
    if start >= end {
        return (0, 0);
    }
    (start, buf.len())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Every task split must yield exactly the file's lines after the header, once each.
    #[test]
    fn tasks_partition_lines() {
        let dir = std::env::temp_dir().join(format!("ss_input_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: Vec<&[u8]> = vec![
            b"h\na\nbb\nccc\n\ndddd\ne",
            b"header\n",
            b"header",
            b"h\n\n\n\nx\n",
            b"h\nabc\r\ndef\n",
        ];
        for (ci, content) in cases.iter().enumerate() {
            let p = dir.join(format!("c{ci}.csv"));
            std::fs::File::create(&p).unwrap().write_all(content).unwrap();
            let expect: Vec<Vec<u8>> = crate::csv::lines(body(content)).map(|l| l.to_vec()).collect();
            for chunk in 1..=content.len() as u64 + 1 {
                for mode in [Mode::Pread, Mode::Mmap] {
                    let mut src = Source::open(&[p.clone()], mode);
                    // re-split with a tiny chunk size
                    src.tasks.clear();
                    let size = src.sizes[0];
                    let mut off = 0;
                    while off < size {
                        let end = (off + chunk).min(size);
                        src.tasks.push(Task { file: 0, off, end });
                        off = end;
                    }
                    let got: Vec<Vec<u8>> = src
                        .par_map(|c| crate::csv::lines(c).map(|l| l.to_vec()).collect::<Vec<_>>())
                        .into_iter()
                        .flatten()
                        .collect();
                    assert_eq!(got, expect, "case {ci} chunk {chunk} mmap={}", mode == Mode::Mmap);
                }
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
