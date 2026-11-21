# ADR-024: One-pass chunk scanner with 64-byte bitmasks (AVX2 on x86)

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** Parsing is about 57% of CPU time ([ADR-021](021-linux-profiling.md)). The parser split
a chunk into lines (one pass), then copied each line into a zeroed 128-byte buffer and scanned all
128 bytes for commas and quotes ([ADR-018](018-bitmask-field-splitting.md)). The research scout
pointed out that since quotes never contain commas, simdjson's quote tracking is not needed.

**Decision.** `csv::for_each_line::<N>(chunk, f)` scans the chunk in place, 64 bytes at a time:
- `block_masks` gives newline, comma and quote bitmasks for a block. On x86-64 with AVX2, which is
  enabled by the Dockerfile's `x86-64-v3`, that is `vpcmpeqb` + `vpmovmskb` on two 32-byte halves;
  elsewhere, portable 64-byte loops.
- A walk over the separator bits (`newline | comma`) cuts fields. Each field's quote count is the
  popcount of its quote bits, carried across block boundaries. A field is valid if it has 0 quotes,
  or exactly 2 with the quotes as its first and last byte. Each `\n` ends a line, which is emitted
  only with exactly N fields. A final line without `\n` is handled.
- Events, clicks and links use it. `lines` + `fields` stay for the small files and as the oracle.

**Safety net.**
- `csv::tests::fuzz_chunk_scanner_matches_reference`: 60k random chunks (up to 400 bytes, dense in
  `\n , "`) at N = 2, 4, 7, plus 3k realistic multi-line chunks with random quoting, broken lines,
  and with or without a trailing newline. Every emitted line must equal `lines` +
  `fields_scalar`.
- Mutation check: accepting a field with 3 or more quotes, and dropping one quote bit, both made the
  test fail. Dropping the "line not empty" check survived, because an empty line has 1 field and
  N ≥ 2, so that check is redundant.
- `scripts/check.sh`: 11/11 unit tests, fixtures in both modes, dev MATCH, seeds 2–3 MATCH.

**Risk.** The AVX2 path is compiled and run only on x86. Locally only the portable path is built
and tested. The AVX2 block was reviewed by hand; the scorer's dev-set check rejects a submission
whose output differs, so a mistake there costs one submission slot, not a wrong score.

**Scored result.** `55032eb` (together with ADR-022 and ADR-023) scored **2.67 s**, against 3.02 s, and
passed the scorer's dev-set check, so the AVX2 path builds and gives correct output on x86.
