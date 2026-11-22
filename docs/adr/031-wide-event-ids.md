# ADR-031: Exact wide keys for event_id: no interning for long event IDs

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** On the new data, 1 in 8 event IDs is the generator's long shape (a letter plus 32
lowercase hex, 33 bytes). Each one was interned ([ADR-029](029-sharded-interner.md)): hash, shard
lock, chain lookup, and a 33-byte copy into an arena. That is about 2.5M occurrences at 20M events,
and event IDs are only ever used for dedup equality (never output or ordered). The research
scout ranked removing it first (est. 0.15–0.25 s).

**Decision.**
- `ids::wide(bytes) -> Wide { hi, lo, tag }`, exact:
  - `tag 0`: a short base-37 key in `lo`;
  - `tag 1..=26`: exactly `[a-z]` plus 32 lowercase hex digits, packed as 128 bits in `hi:lo`, with
    the letter in the tag;
  - `tag 255`: anything else, through the interner.

  The shapes are disjoint by length and character set, and each is injective, so equal keys
  ⇔ equal IDs. All-zero never occurs, so it marks empty slots.
- `table::Table<K, V>` is generic over a `SlotKey` (empty marker and hash); `FlatMap = Table<u64, _>`
  as before, and `WideMap = Table<Wide, _>` for dedup.
- `RawEv` holds the wide event_id as separate fields, with etype and the valid flag packed in one byte:
  40 → 48 bytes (not 56). Event buckets use `partition::bucket_wide`.
- User and click IDs are unchanged (they still need byte order and output).

**Verification.**
- New `ids::tests::fuzz_wide_is_exact`: 4,500 IDs, pairwise. Mixed shapes: well-formed long hex,
  near misses (31/32/34 hex digits, uppercase or digit prefix, one uppercase-hex or non-hex
  character), short, arbitrary bytes, and deliberate duplicates. Equal keys iff equal bytes, and no
  all-zero key.
- `scripts/check.sh`: 14/14 unit tests, fixtures in both modes, dev MATCH against the new official
  answer, new-generator seeds 2–3 MATCH DuckDB.

**Scored result.** `f06e24e` (with ADR-030) scored **2.46 s**, against 2.62 s for `703f333`: −0.16 s,
inside the research scout's 0.15–0.25 s estimate.
