# ADR-013: Ask the kernel to read every input file ahead (`madvise WILLNEED`)

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** The same binary on the same 5M-event data took 1.35 s on its first run and 0.70 s on
its second. The first run had 29,087 page faults and 0.60 s of system time; the second had 22.
The difference is the file cache: with mmap, each first touch of a page stalls a parsing thread
until the disk delivers it. Whether the scorer's files are cached is unknown, so cold reads may
dominate a scored run.

**Options considered.**
- **A. mmap plus kernel hints:** `madvise(WILLNEED)` (start reading in the background), or
  `MAP_POPULATE` (load everything before `mmap` returns).
- **B. Parallel `pread` into our own buffers:** full control over I/O depth, but a new reading
  layer and an extra copy.

**Decision.** Option A, using `WILLNEED` plus `SEQUENTIAL` on every events, clicks and links file,
issued for all files before any parsing starts ([input.rs](../../src/input.rs)). No
`MAP_POPULATE`: it blocks until a file is fully loaded, so parsing could not overlap the reads.
Events and clicks parse concurrently again (`rayon::join`).

**Why.** About 10 lines, with no change to any rule logic. Reading and parsing overlap: links (small)
parse while the event files stream in.

**Verification.** 5/5 unit tests, 36/36 fixtures, dev set MATCH, and 5M output identical to the
DuckDB reference.

**Measurements (macOS laptop, 4 threads, 5M events, warm cache, 3 alternating runs).**

| | Run 1 | Run 2 | Run 3 |
| --- | --- | --- | --- |
| Plain mmap | 1.29 s | 0.92 s | 0.68 s |
| `WILLNEED` | 0.76 s | 0.81 s | 0.66 s |

Never slower. macOS cannot reproduce a cold Linux page cache (clearing the cache needs
`sudo purge`), so the scorer is the real measurement.

**Consequences.** If cold reads still dominate the scored time, option B is next, on its own
branch, compared against this one.
