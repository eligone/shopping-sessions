# ADR-022: Ask mimalloc for huge OS pages (`MIMALLOC_ALLOW_LARGE_OS_PAGES=1`)

- **Status:** Accepted (branch `partitioned`); to be judged by a scorer submission
- **Date:** 2026-11-21

**Context.** The run touches several GB of heap. With 4 KB pages that means hundreds of thousands
of first-touch page faults and many TLB misses. In Docker's Linux VM, transparent huge pages are
`always`, so faults were already low (9k–18k at 5M) and the setting could not be judged locally
([ADR-021](021-linux-profiling.md)). Many cloud kernels use THP `madvise`, where a process only gets
2 MB pages if it asks; the scorer's setting is unknown. The research scout ranked this first: tiny
effort, possibly a large effect on the scorer.

**Decision.** `run.sh` exports `MIMALLOC_ALLOW_LARGE_OS_PAGES=1`, so mimalloc asks the kernel for
2 MB pages for its memory. It falls back to normal pages when huge pages are unavailable, so the
setting cannot cause a failure.

**Verification.** Ran the binary through `run.sh` in the Linux container on the dev set: correct
output files, 0.18 s.

**Measurement plan.** This commit differs from `main` (`da351f1`, 3.02 s) only in `run.sh` and the
docs, so its score is a clean A/B test of the setting.
