# ADR-002: Parse CSV lines ourselves, not with DuckDB's CSV dialect handling

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** `rules.md` defines its own dialect: only `\n` ends a line, `\r` is data, quotes only
around whole fields, `""` is null, wrong field counts are dropped. DuckDB's reader reads `""` as
`''`, repairs or rejects malformed lines its own way, and **splits lines at `\r` even with
`new_line='\n'`** (verified: fixture E20 broke into extra rows).

**Decision.** Read each line as one opaque string, then:
1. keep it only if it fully matches `field(,field){n-1}` where a field is `"[^",]*"` or `[^",]*`;
2. split on `,`, strip quotes, turn empty into NULL.

**Why.** The regex is the dialect, written once, with no reader quirks in between. Empty lines,
stray quotes and wrong field counts all fail the same match.

**Consequences.** Every rule about quoting, spaces and `\r` holds by construction; fixtures
E13, E15, E19, E20 and L3 pass.
