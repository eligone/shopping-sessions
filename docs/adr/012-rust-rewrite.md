# ADR-012: Rewrite the pipeline in Rust, with integer IDs and one parse pass

- **Status:** Accepted (branch `rust`)
- **Date:** 2026-11-21
- **Supersedes, on this branch:** [ADR-001](001-duckdb-engine.md) as the engine; the DuckDB
  pipeline stays in `pipeline/` as the reference for differential tests.

**Context.** The DuckDB version scored 37.63 s on about 20M practice events while the leaders run
in about 1 s. Profiling showed the time going into a general SQL engine: string IDs in every
sort and join, five or six materialized intermediate tables, regex-based line checks, and about
0.5 s of Python startup before any work.

**Data measured (5M events).** Lines average 76 bytes (100 at most); every ID is at most 9 bytes;
79% of lines contain a quote; timestamps are 58% F1, 32% F3 and 10% F2; event files range from
header-only up to 1/5 of all rows.

**Decision.** A single Rust binary (`src/`), one module per stage:

| File | Role |
| --- | --- |
| `main.rs` | env, thread pool, stage order, per-stage timings to stderr, output assembly |
| `input.rs` | mmap every CSV, split into about 4 MB line-aligned chunks, skip each file's header |
| `csv.rs` | the rules' dialect in one scan per line, zero-copy fields, `\r` is an ordinary byte |
| `ts.rs` | F1/F2/F3 → epoch ms with every validation rule; unit-tested on every rules case |
| `ids.rs` | an ID of up to 15 bytes packed into a `u128` (bytes + length): exact, and integer order = byte order; longer IDs use an exact shared table |
| `links.rs` | union-find over linked IDs; each person named by the byte-wise smallest ID |
| `events.rs` | one parse pass into 64 hash buckets by `event_id`; per bucket keep the smallest `ingest_ts`, then validate and drop late events |
| `clicks.rs` | parse and validate, sort by (person, ts, click_id), drop adjacent copies |
| `sessions.rs` | sort by (person, ts); one scan finds sessions and sums each person-day |
| `attribution.rs` | per person, walk sessions and clicks together for credit; merged click ranges for assists |
| `output.rs` | Parquet via the `parquet` crate without Arrow: `day` is INT32 DATE |

**Why.**
- Packing IDs into integers removes string work after parsing: dedup, person lookup and sorting
  all compare `u128`s, and the byte-wise ordering rule falls out of integer order.
- Each byte of input is scanned about once, in parallel across chunks.
- Ties need no special care: after sorting, equal timestamps have a gap of 0 and stay in one
  session. That's the bug [ADR-007](007-range-frames-for-ties.md) fixed in SQL, here excluded by
  design.
- Click copies are identical, so validating before dedup gives the same result as the rules'
  order and lets dedup happen for free after the sort.

**Build.** A multi-stage Dockerfile: `rust:1.85-bookworm` compiles with LTO and `target-cpu=x86-64-v3`
(AVX2) on amd64, and the binary ships on `debian:bookworm-slim`. AVX2 rather than AVX-512, because
an unsupported instruction would crash the run outright.

**Verification.**
- 5 unit tests (dialect, timestamps, ID ordering)
- 36/36 fixtures
- an exact match with the dev answer
- byte-identical output to the DuckDB reference on the 5M set and on seeds 2–10 at 1M events
  (`scripts/diff_test.sh`)

**Results (laptop, 4 threads).**

| Data | DuckDB | Rust |
| --- | --- | --- |
| 1M events | 1.5 s | **0.24 s** |
| 5M events | about 7 s | **1.34 s** (1.4 GB peak memory) |

**Consequences.** Events and clicks parsing is now about 85% of the run (1.13 s of 1.34 s at 5M)
and uses the threads poorly (2.1 CPU-seconds over 1.35 s on 4 threads). That's the next target.
