# ADR-036: Write user_daily strings from one shared buffer

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** The research scout ranked the output writer as the surest next win. user_daily (about
2.3M rows at 20M) created one heap allocation per row for `user_id`, unpacking long names under a
shard lock every time, and cloned the category name `Vec` per row for `top_category`.

**Decision.**
- Per output file, append every row's user_id bytes into one `Vec<u8>`, turn it into a
  `bytes::Bytes`, and give each row a zero-copy `slice` (Parquet's `ByteArray` wraps `Bytes`).
- A person's rows are adjacent, so each name is unpacked once (`ids::append_bytes`, no allocation of
  its own).
- Category names become shared `ByteArray`s once; a row clones one (a reference-count bump).
- `bytes = "=1.12.2"` is added as a direct dependency; it was already in Cargo.lock through parquet.

**Verification.** `scripts/check.sh`: 16/16 unit tests, fixtures in both modes, dev MATCH, seeds 2–3
MATCH; dev also MATCH in mmap mode.

**Measurement (light, 1M, Mac, 3 runs).** user_daily output 0.006 s → 0.003 s; campaign_daily
0.004–0.005 s → 0.002 s. About 0.1 s at 20M if it scales with rows.
