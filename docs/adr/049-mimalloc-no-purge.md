# ADR-049: Keep freed memory in the allocator (`MIMALLOC_PURGE_DELAY=-1`)

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
Since ADR-048, each event bucket frees its parsed copies early, so the memory can be reused. But by
default mimalloc returns freed memory to the OS after 10 ms, and reusing it then costs fresh page
faults and zeroing again.

## Decision
`run.sh` exports `MIMALLOC_PURGE_DELAY=-1`, so freed memory stays with the allocator for reuse. Only
allocator settings change, so results are unchanged. The benchmark scripts use the same setting.

## Measurements (VM, 20M, 5 runs each, same binary `2bcb7f5`)
| Mode | Default | No purge |
|---|---|---|
| THP madvise | 1.929 s | 1.876 s |
| THP never | 2.395 s | 2.377 s |
(Tested before ADR-048, when nothing was freed early, it made no difference.)
