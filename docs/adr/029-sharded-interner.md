# ADR-029: Shard the escaped-ID interner for the generator's long IDs

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** The organizers updated rules.md: "One in eight distinct `user_id`, `event_id` and
`click_id` values carries a 32-character hexadecimal suffix ... pipelines must treat IDs as opaque
variable-width strings." No processing rule changed (checked line by line against our copy). Those
IDs are about 41 bytes, too long for the 12-character base-37 key ([ADR-023](023-u64-ids-u32-times.md)),
so they took the escaped path: one global `Mutex<HashMap<Vec<u8>, u64>>` meant for rare oddities.
With 1 in 8 IDs there, millions of lookups per run queued on one lock.

Scores on the new data: `c203fa6` 2.45 s before the change → **6.94 s**; the batch `87780b9` **6.63 s**.
The leaderboard top only moved from 0.28 to 0.51 s, so the slowdown was largely ours.

**Decision.** The interner is sharded 1024 ways by a fixed-seed `ahash` of the bytes. Each shard has
its own lock, a byte arena (no allocation per ID), spans, and a hash → first-ID map with collision
chains. Equality is always decided by comparing bytes, so the interner stays exact. The key is
`ESC | shard << 32 | index`.

**Verification.**
- `ids` tests: the fuzz test now includes 41-byte IDs and re-packing consistency. A new test has 8
  threads interning 20k overlapping long IDs at once, and all must agree on every key and unpack
  exactly.
- `scripts/check.sh`: 13/13 unit tests, fixtures in both modes, dev MATCH, seeds 2–3 MATCH.
- **Simulated long-ID data:** the 1M dev set rewritten so that 1 in 8 distinct IDs gets a
  consistent 32-hex suffix (165,454 of 1,324,503). Rust matches DuckDB exactly.
- **Timing, light, 3 alternating runs:** single lock 0.31–0.43 s, sharded 0.14–0.20 s, about
  2.2–2.5× faster.

**Validated on the real new generator (same day).** The organizers' new `generator.zip` and
`dev-answer.zip`: long IDs are a prefix letter plus 32 lowercase hex digits (33 bytes), chosen per
ID by `scrambled & 7 == 0`; 12.3% of event and user ID values in the 1M dev set. With the
regenerated dev set, `scripts/check.sh` passes: dev MATCH against the new official answer (118,107
user-days), and new-generator seeds 2–3 MATCH DuckDB. Timing on that dev set: single lock
0.34–0.51 s, sharded 0.12–0.15 s (2.7–3.5×).

**Scored result.** `703f333` scored **2.62 s** on the new hidden data, against 6.63 s for `87780b9`
(single lock): 2.5× faster, inside the 2.2–3.0 s estimate. A rules-auditor review found no issues.
