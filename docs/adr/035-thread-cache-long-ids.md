# ADR-035: Per-thread cache in front of the interner for long hex-shaped IDs

- **Status:** Rejected (removed after scoring slower)
- **Date:** 2026-11-22

**Context.** The parse ablation ([ADR-034](034-parse-ablation-v2.md)) put user_id packing at about 17%
of the events parse, almost all of it the interner for long (33-byte hex) user IDs. A person has
about 30 events, so the same long user ID is hashed, locked and compared about 30 times per run.

**Decision.** `ids::pack` first tries base-37. For a hex-shaped ID it looks up the exact Wide form in a
thread-local `WideMap<Key>`; only on a miss does it go to the shared interner, and then it
remembers the key. The cache is thread-private, so it needs no lock. It is exact because the Wide
form is exact ([ADR-031](031-wide-event-ids.md)) and the interner is deterministic within a run.
`ids::pack_once` skips the cache for click_id, which almost never repeats.

**Verification.**
- New `ids::tests::cached_pack_matches_uncached`: 6 threads, repeated lookups of long, short and
  uppercase-prefixed IDs; `pack == pack_once`, and the key unpacks to the same bytes.
- `scripts/check.sh`: 16/16 unit tests, fixtures in both modes, dev MATCH, seeds 2–3 MATCH.

**Measurement (light, 1M, Mac).** Inconclusive: the user-pack step still costs about +0.009 s, and the
totals of 0.111–0.129 s are in the previous range. An uncontended lock is cheap on the Mac; the
cache may matter more under 8-thread contention at 20M on the scorer. Judged by submission, or
dropped if the research scout advises against it.

**Revision (research scout review).** An unbounded per-thread map would grow to about 170k entries
(about 10 MB per thread) and miss DRAM anyway. It is now a fixed, direct-mapped table of 2^15 slots
(about 1 MB, sized for L2): a slot holds the exact Wide form and its key, and is overwritten on a
miss. An empty slot's all-zero Wide can never equal a hex-shaped ID's (tag 1..=26). Checks still
pass. Still no visible local gain (uncontended locks are cheap on the Mac); the scout estimates
0.05–0.1 s on the scorer.

**Scored result and removal.** `bb03de6` (this cache plus [ADR-036](036-shared-buffer-output.md)) scored
**2.51 s** against 2.40 s for `7f9f4ca`. The likely cause is this cache:
- every long user ID now pays a 32-digit hex decode plus a hash just to probe it, even on hits;
- the 1 MB per-thread table is as big as a Cascade Lake core's whole L2 (1 MB), so it can evict the
  parser's own working data.

Removed: `pack` is the plain base-37-or-interner path again, and `pack_once` is an alias. The next
submission (ADR-036 alone against `7f9f4ca`) tests this.

**Lesson.** "Fits in L2" must count everything else that needs L2 at the same time.
