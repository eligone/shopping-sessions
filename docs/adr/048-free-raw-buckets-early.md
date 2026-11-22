# ADR-048: Free each event bucket's parsed copies as soon as it is routed

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
The scorer gained only 0.05 s (1.68 → 1.63 s) from a 0.22 s VM gain. A step-back test simulated a
machine without transparent huge pages (THP set to `never`, mimalloc large pages off). Wall time went
from 1.95 to 2.62 s, minor page faults from 14k to 800k, and system time from 0.8 to 3.2 s. If the
scorer's machine behaves like this, a large share of its time is the kernel zeroing fresh pages, and
cutting CPU work doesn't touch that. So we now benchmark in both THP modes (`vmbench2.sh`).

## Decision
`dedup_route` transposes the parsed copies to `[bucket][chunk]`, moving the vectors without copying.
Each bucket task owns its pieces and drops them when it finishes, so the allocator can hand that
memory to the routed output of later buckets instead of fresh pages. The routed events and clicks are
still not freed before exit (ADR-020): doing the same in the partition stage was slower in both modes
(1.973 → 2.007 s with THP, 2.599 → 2.654 s without).

## Why it is exact
Only who owns the memory and when it is freed changes. The pieces are read in the same chunk order,
so the dedup indexes and tie-breaks are identical. `scripts/check.sh full`: all checks passed.

## Measurements (VM, 20M, 5 paired runs per mode, two rounds)
| Mode | Before (`654aef4`) | After |
|---|---|---|
| THP madvise (our VM) | 1.967 / 1.926 s | 1.948 / 1.935 s |
| THP never (no huge pages) | 2.541 / 2.493 s | 2.490 / 2.425 s |
