# ADR-053: Software write-combining in the events parse scatter

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
The events parse writes each 44-byte record straight into one of 512 event-id buckets, so
consecutive writes land in 512 different places in memory. That spreads writes over many cache
lines and pages at once (TLB pressure: the small cache of address translations).

## Decision
`parse_chunk` stages records in a per-thread area of 16 records per bucket (512 × 16 × 44 B ≈
360 KB, reused across chunks, so it stays in L2). A full slot is copied out in one burst; leftovers
are flushed at the end of the chunk. Order within each bucket is unchanged, so dedup ties behave
as before. `SS_WC` selects the mode: 0 = direct push (old), 1 = staged + memcpy (default),
2 = staged + non-temporal 4-byte streaming stores.

## Measurements (VM, 20M, run.sh environment, THP madvise, 5 alternating rounds)
| Round | base (`f6617a6`) | SS_WC=1 | SS_WC=2 |
|---|---|---|---|
| 1 | 1.987 | 1.943 | 1.966 |
| 2 | 2.040 | 1.955 | 1.979 |
| 3 | 2.002 | 1.953 | 1.967 |
| 4 | 1.985 | 1.937 | 1.957 |
| 5 | 2.002 | 1.928 | 1.997 |
| median | 2.002 s | **1.943 s** | 1.967 s |

Mode 1 is faster in all 5 rounds (−0.06 s). Streaming stores helped less: dedup reads the buckets
soon after, and 4-byte streaming stores cost more instructions.

Stage laps at 20M (mode 2 run): events parse 0.948 s, dedup+route 0.542 s, partitions 0.349 s,
output 0.067 s, links union-find 0.364 s (overlapped with the parse).

## Verification
36/36 fixtures in both read modes and an exact dev-set match, for modes 1 and 2. `check.sh full`
was NOT run (owner asked to skip it for time).

## Not measured
THP `never` mode, and the 60M set.
