# ADR-044: SWAR timestamp checks (F1/F2 time of day, short F3)

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
Parsing `ts` and `ingest_ts` cost about 0.29 s of the 1.09 s events parse on the VM.
The month-table fast path (ADR-039) still checked "DD", "T", "HH:MM:SS" byte by byte
with a branch per byte, and F3 (epoch milliseconds) looped over each digit.

## Decision
"SWAR" (SIMD within a register): treat 8 bytes in one 64-bit register as 8 lanes.
- F1/F2: load bytes 8..16 and 11..19 as u64s, XOR with the templates "00T…" and
  "00:00:00". A byte becomes 0..=9 exactly when it was a digit, and 0 exactly when it
  was the expected separator. One branch-free test (`all_le9`) checks all digits at once.
- F3 of at most 16 characters: left-pad with '0' to 16 bytes (leading zeros are allowed
  and do not change the value), check both halves with `all_le9`, and convert each half
  of 8 digits with the multiply trick (`swar8`). Longer F3 strings keep the old loop.

## Why it is exact
- `all_le9`: a byte with its top bit set fails directly; any other byte plus 0x76 cannot
  carry into its neighbour, and has its top bit set exactly when the byte is ≥ 10.
- XOR with '0' maps only '0'..='9' to 0..=9, and XOR with ':' or 'T' gives 0 only for that byte.
- New fuzz tests: `fuzz_f3_short_matches_loop` (200k random digit strings with one planted
  random byte) and a stronger `fuzz_fast_datetime_matches_full` (a random byte planted anywhere in
  "DDTHH:MM:SS", 400 random windows).

## Measurements (AWS m5.2xlarge, 20M, 5 alternating runs)
| Binary | Median |
|---|---|
| `40c07ee` code | 2.148 s |
| ADR-044 | 2.053 s |
Faster in every run (−0.095 s). `scripts/check.sh full` (including calendar-edge seeds): all checks passed.
