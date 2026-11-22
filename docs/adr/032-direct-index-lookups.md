# ADR-032: Direct-index product and campaign lookups; fast scanner for the small CSVs

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** Every event looks up its product and every click its campaign. Products went through
base-37 encoding plus a hash probe into about 2 MB (often a cache miss). Campaign IDs (`c_306`)
contain `_`, so they fell to a std `HashMap<Vec<u8>, _>` with SipHash (slow by design) plus a byte
comparison. products.csv and campaigns.csv were parsed with the per-line parser.

**Decision.**
- `src/numindex.rs` `NumIndex`: the common prefix is learned from the file itself (the non-digit
  prefix of the first ID followed by a number), so nothing is hard-coded. An ID that is exactly
  that prefix plus a canonical decimal (ASCII digits, no leading zero unless "0", at most 7 digits,
  under 2^22) maps one-to-one to its number, so a `Vec<u16>` indexed by the number answers exactly:
  - shaped and present → the value;
  - shaped and absent → not in the file;
  - any other ID → the existing exact general path.

  First row wins on repeats. About 100 KB for 50k products, so it stays in cache.
- products.csv and campaigns.csv are read with `csv::for_each_line` instead of `lines` + `fields`.

**Verification.**
- `numindex::tests::fuzz_matches_hashmap`: 30 random catalogs, 90k queries, mixing leading zeros,
  "0", numbers past the limit, trailing letters, other and empty prefixes. Direct index plus
  fallback equals a first-wins `HashMap` by bytes.
- Mutation check: allowing leading zeros (so `p01` would hit `p1`'s slot) fails the test.
- `scripts/check.sh`: 15/15 unit tests, fixtures in both modes, dev MATCH, seeds 2–3 MATCH.

**Measurement (light, 1M, Mac, 4 alternating runs, together with ADR-033).** Setup 0.010–0.014 s →
0.007–0.009 s; totals 0.124–0.195 s → 0.113–0.135 s. A small gain, within local noise.
