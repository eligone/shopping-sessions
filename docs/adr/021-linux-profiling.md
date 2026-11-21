# ADR-021: Profile on Linux with perf in Docker

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** macOS timings were misleading for the scorer: a different CPU (ARM), 16 KB pages, no
transparent huge pages, and heavy memory pressure. The scorer reports only a total time.

**Decision.** `Dockerfile.profile` (separate from the submission `Dockerfile`) builds the pipeline
in Linux with `perf` and line-level debug info. `scripts/linux_profile.sh [DATA] [ENV=..]` copies
the dataset into the VM first (Docker on macOS serves bind mounts through a slow file-sharing
bridge, which more than doubled the parse and dedup times in a first attempt), then runs a warm-up,
the stage timings, `perf stat`, and a `perf record` function profile.

**Findings (5M events; Docker Desktop VM: 8 CPUs, 4 GB, aarch64, kernel 6.10).**
- Warm totals 0.55–0.65 s, with 6.5–6.9 of 8 CPUs busy: parallelism is fine.
- **Self time by function:**

  | Function | Share |
  | --- | --- |
  | `csv::fields` | 19.2% |
  | `events::parse_chunk` | 17.0% |
  | clicks parse closure | 11.8% |
  | dedup closure (likely) | 9.9% |
  | kernel copy for `pread` | 4.4% |
  | line splitting | 4.1% |
  | rayon | 4.0% |
  | sorts | about 7% |
  | timestamps | about 4.7% |

  So **parsing is about 57% of the CPU time**.
- **Transparent huge pages are `always`** in this VM, so there were only 9k–18k page faults; fault
  handling never showed up above 1.5%. `MIMALLOC_ALLOW_LARGE_OS_PAGES=1` was inconclusive here
  (0.52/0.52/0.71 s against 0.62/0.66/0.56 s). It can only matter where THP is `madvise`, which
  is unknown for the scorer.

**Consequences.** Next targets: smaller records and SIMD scanning (parsing). The huge-pages setting
gets tried on the scorer. An x86 Linux VM matching the scorer (GCP n2) would remove the remaining
mismatch (ARM CPU).
