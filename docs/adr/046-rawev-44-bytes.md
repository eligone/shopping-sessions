# ADR-046: 44-byte parsed event records

**Status:** Accepted (branch `lowlevel`), measured together with ADR-045
**Date:** 2026-11-22

## Context
Every parsed event copy (`RawEv`, 48 B) is written during the parse and read back by dedup and
routing: about 1 GB each way at 20M. The routing stage is mostly this memory traffic (a probe
showed that the persons lookup is only about 0.05 s of it).

## Decision
- `ts` is stored as a u32 offset from the window start, and only for valid rows (the only rows that
  use it). A valid ts is inside the window, and `main` asserts that the window is under 49 days.
- The struct is `#[repr(C, packed(4))]`, so the u64 fields need only 4-byte alignment. That makes
  44 B instead of 48 B. Fields are only ever read by value, never by reference.
- `ingest_ts` stays a full i64, because dedup compares it exactly for every copy, valid or not.
  That is why the planned 40 B layout was not used.

## Measurements (AWS m5.2xlarge, 20M, 5 alternating runs)
| Binary | Median |
|---|---|
| ADR-044 (`c16f870`) | 2.075 s |
| ADR-044 + ADR-045 + ADR-046 | 2.009 s |
Faster in every run. `scripts/check.sh full` (with the calendar-edge seeds): all checks passed.
