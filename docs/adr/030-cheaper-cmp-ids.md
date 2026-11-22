# ADR-030: Cheaper byte-wise ID comparison (`cmp_ids`)

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** With 1 in 8 IDs escaped (33-byte hex-suffixed IDs, [ADR-029](029-sharded-interner.md)),
`cmp_ids` unpacked both sides whenever either was escaped: two shard locks and two heap
allocations per call, even when both keys were identical. The rules auditor flagged two hot uses:
the click sort's tie-break (mostly identical duplicate copies) and links naming (about 23% of
comparisons involve a long ID).

**Decision.**
- Equal keys return `Equal` at once: keys are exact, so equal keys mean equal IDs.
- **Short vs escaped:** decode the short key into a stack buffer and compare it against the
  escaped bytes under that one shard's lock. No allocation.
- **Escaped vs escaped:** copy one side (its lock is released first), then compare under the
  other's lock, so at most one lock is ever held and deadlock is impossible.
- `unpack` reuses the same helpers.

**Verification.**
- The `ids` fuzz tests compare `cmp_ids` with plain byte order over mixed short and escaped IDs,
  including 41-byte ones.
- Mutation check: reversing the escaped-vs-short comparison fails 2 tests.
- `scripts/check.sh`: 13/13 unit tests, fixtures in both modes, dev MATCH against the new official
  answer, seeds 2–3 MATCH.
