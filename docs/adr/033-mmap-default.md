# ADR-033: Read inputs through mmap on the scorer (`SS_READ=mmap` in run.sh)

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** [ADR-014](014-pread-read-mode.md) found mmap about 10% faster on warm runs and pread
about 25% faster on cold ones. A/B at the time: a tie (4.06 vs 4.05 s) while other costs dominated.
Scores are the median of 3 runs on a warm page cache, and parsing is now much faster, so the copy
pread makes (about 4% of CPU in the Linux profile, [ADR-021](021-linux-profiling.md)) is a larger
share.

**Decision.** `run.sh` exports `SS_READ=mmap`. The pread mode stays available, and both modes are
checked by the fixtures.

**Verification.** Dev set in mmap mode: MATCH against the new official answer. Fixtures pass in both
modes.
