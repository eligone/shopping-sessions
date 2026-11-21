# ADR-018: Bitmask field splitting, guarded by a differential fuzz test

- **Status:** Accepted (branch `rust`)
- **Date:** 2026-11-21

**Context.** [ADR-017](017-parse-ablation.md) measured field splitting (`csv::fields`) as the largest
single cost of the events parse (35%, +0.22 s at 10M). It examined bytes one at a time, scanning
for commas and then again for quotes.

**Decision.**
- `csv::fields` now copies a line of up to 128 bytes into a 128-byte buffer and computes two
  128-bit masks (bit i = byte i is a comma / a quote) with fixed-size 64-byte loops the compiler
  can vectorize. Then:
  - field count is `count_ones` of the comma mask, which must be N−1;
  - boundaries come from `trailing_zeros`;
  - the quote rule is that a field's quote bits must be 0, or exactly its first and last byte.
- Lines over 128 bytes (never generated, but possible) use the old byte-at-a-time parser, kept as
  `fields_scalar`.

**Safety net.**
- `csv::tests::fuzz_fields_matches_scalar` checks the two parsers give identical results on 1M
  random lines (commas, quotes, `\r`, tabs, spaces, multibyte UTF-8, lengths crossing 64 and 128
  bytes) for N = 2, 4 and 7, plus 200k well-formed 7-field lines with random quoting.
- **Mutation check:** deliberately accepting a one-byte `"` field made the test fail immediately,
  so it can catch quote-logic bugs.
- Plus `scripts/check.sh`: 9/9 unit tests, fixtures in both modes, dev MATCH, seeds 2–3 MATCH.

**Measurements (`SS_STAGE=parsebench`, 10M events, 4 threads, macOS ARM, 3 rounds).**

| | Scalar | Bitmask |
| --- | --- | --- |
| Field splitting (variant 2 − variant 1) | +0.22 s | +0.17–0.19 s |
| Full events parse | 0.60–0.62 s | 0.52–0.55 s |

**Why the gain is modest.**
- ARM NEON has no movemask instruction, so building the bitmasks takes extra steps on the Mac. The
  scorer's x86 CPUs have one (`pmovmskb`, AVX2), so the gain there may be larger; unverified.
- Each line is copied into a zeroed 128-byte buffer and all 128 bytes are scanned (lines average
  76 bytes).

**Consequences.** A next step, if worth it: compute the masks once per 4 MB chunk (no per-line copy),
then walk lines and fields from the masks.
