# ADR-052: 512 partitions and event-id buckets instead of 256

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
With 256 partitions, each event-id bucket's dedup table (about 2 MB) and each person partition's
grouped data (about 1.9 MB) are larger than the 1 MB per-core L2 cache.

## Decision
`PART_BITS = 9`: 512 event-id buckets and 512 person partitions. That halves each working set.
Bucket bits 28..37 of the mix now overlap the dedup fingerprint's lowest bit (bit 36). The
fingerprint is only a filter, and a match is always confirmed by comparing the full event id, so
results are unchanged.

## Measurements (VM, 20M, 5 paired runs per mode, two rounds)
| Mode | 256 | 512 | 1024 |
|---|---|---|---|
| THP madvise | 1.966 / 1.936 s | 1.954 / 1.957 s | 2.044 s |
| THP never | 2.538 / 2.561 s | 2.409 / 2.450 s | 2.551 s |
That is about −0.11 to −0.13 s without huge pages and neutral with them. 1024 is worse in both modes.
`scripts/check.sh full`: all checks passed.
