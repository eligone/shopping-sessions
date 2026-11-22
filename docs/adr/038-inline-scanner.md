# ADR-038: Force-inline the chunk scanner into its callers

- **Status:** Accepted (branch `lowlevel`)
- **Date:** 2026-11-22

**Context.** The Intel profile showed 12.2% of CPU in a generic `FnMut::call_mut`: the per-line
callback passed to `csv::for_each_line` was being called, not inlined. That costs a call per line
and blocks optimization across the scanner and the column decoding. The research scout ranked
this first among cheap fixes.

**Decision.** `#[inline(always)]` on `csv::for_each_line`, so each caller (events, clicks, links, small
CSVs) gets its own copy with the callback inlined.

**Verification.** Full check on the Intel VM passes (16/16, fixtures, dev answer, seeds 2–10).

**Measurement (Intel VM, 20M, 5 alternating runs).** Median 2.719 s → **2.691 s**; faster in all 5
runs. Smaller than hoped (the scout estimated 50–150 ms); the larger change in the same direction
is passing field spans instead of the `Option` array, and the two-stage scanner (C1).
