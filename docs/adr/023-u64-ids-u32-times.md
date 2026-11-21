# ADR-023: 64-bit base-37 IDs and 32-bit event times

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** The Linux profile ([ADR-021](021-linux-profiling.md)) put about 57% of CPU time in
parsing, and the research scout ranked shrinking every record first. IDs were 128-bit keys (bytes
plus length), so the raw event was 56 B and the routed event 32 B.

**Decision.**
- **IDs → u64, base 37** (`ids::try_encode`). An ID of 1–12 characters from `0-9a-z` maps each
  character to 1..=36 (digits before letters, as in ASCII) and right-pads with 0 to 12 places.
  37^12 < 2^63. The encoding is exact, and integer order equals byte-wise order, padding sorting
  first, so a prefix sorts first.
- **Every other ID** (upper case, spaces, `_`, multi-byte UTF-8, over 12 bytes) is interned in the
  shared escaped table with the top bit set; `cmp_ids` compares by bytes when either side is
  escaped.
- **Product and campaign lookups take the raw bytes.** Encodable IDs are looked up by key, others in
  a byte-keyed map, so case-variant dirt is never interned.
- **Ev::ts is a u32 offset from window_start.** main asserts the window is under 2^32 ms (49 days);
  the generator always writes 14 days. Clicks keep i64 times (their window starts 7 days early).
- **Sizes:** RawEv 56 → 40 B, Ev 32 → 24 B, Click 48 → 32 B. Unit tests pin them.

**Verification.**
- New fuzz test `ids::tests::fuzz_order_and_roundtrip`: 3,000 random IDs, mixed encodable and
  escaped, checking round-trips, exactness and byte-wise order.
- `scripts/check.sh`: 10/10 unit tests, 36/36 fixtures in both modes, dev MATCH, seeds 2–3 MATCH.
- Not benchmarked locally (heavy runs are off the laptop); the scorer measures it.
