# ADR-037: Branch-free, table-driven ID encoding (base-37 and hex)

- **Status:** Accepted (branch `lowlevel`)
- **Date:** 2026-11-22

**Context.** The first Intel profile (AWS m5.2xlarge, Xeon 8175M, 20M events) put `ids::wide` at 9.5%
of CPU, plus base-37 encoding inlined into the parsers. `try_encode` did a three-way `match` per
character with an early return, then a second loop multiplying by 37 once per padding place.
`try_hex_shape` did a `match` with `?` per hex digit. Mixed ID data makes those branches
mispredict.

**Decision.**
- `DIGIT: [u8; 256]` (const-built) maps each byte to its base-37 digit, with 0 for invalid. The loop
  accumulates `v = v*37 + d` and an `all &= (d != 0)` flag with no early exit, and pads with one
  multiply by `POW37[12 - n]`.
- `HEX: [u8; 256]` maps lowercase hex to 0..=15 and everything else to 0x10. The two 16-digit loops
  OR the values into `bad` and check once.
- Semantics are unchanged: same keys and same shapes.

**Verification.** Full check on the Intel VM (`scripts/check.sh full`): 16/16 unit tests (the ID fuzz
tests compare against byte order and exactness), 36/36 fixtures in both modes, the new official dev
answer, and seeds 2–10 MATCH DuckDB.

**Measurement (Intel VM, 20M events, run like run.sh, 5 alternating runs).** `a31e315` median 2.987 s
→ **2.698 s** (−0.29 s, −10%); faster in all 5 runs.
