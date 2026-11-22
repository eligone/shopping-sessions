# ADR-050: Dictionary-encoded output columns (EXPERIMENT for the scorer)

**Status:** Withdrawn before scoring (reverted). ADR-051 found the real hidden cost, and this change was 0.02 s slower on the VM.
**Date:** 2026-11-22

## Context
The scorer tracks VM gains weakly: VM 2.15 → 1.93 s gave scorer 1.68 → 1.62 s. Something on the
scorer costs time that the VM doesn't show. One candidate: the output is 104 MB of uncompressed,
plain-encoded Parquet (ADR-019 turned dictionaries off for CPU). If the scorer's output directory is on
slower storage, or the output is copied or read back, file size could matter there.

## Decision
Dictionary encoding is on for every column except `user_id`, which is mostly unique. The small,
repeated values (day, counts, revenue, top_category) become short dictionary codes. Values are unchanged.

## Measurements (VM, 20M)
| Variant | Output size | VM median (5 runs) |
|---|---|---|
| Plain (`73af7c2`) | 104 MB | 1.942 / 1.967 s |
| Dictionary on all columns | 28 MB | 1.993 s |
| Dictionary except `user_id` (this ADR) | 45 MB | 1.990 s |
`scripts/check.sh full`: all checks passed. On the VM it is about 0.02 s slower. Only a scorer run can
show whether smaller files win there. Revert if the score is not better than 1.62 s.
