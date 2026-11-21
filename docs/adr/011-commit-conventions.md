# ADR-011: Commit conventions: no AI co-authors, ADRs updated with every commit

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** Earlier commits carried a `Co-Authored-By: Claude` trailer, which makes GitHub list
Claude as a contributor. The repo owner wants to be the only author. Design decisions were also
being recorded after the fact rather than alongside the code.

**Decision.**
- No commit message names an AI tool as co-author (Claude, Copilot or any other). `CLAUDE.md`
  states this for Claude Code, and the local `commit-msg` hook strips such trailers as a backstop.
- Every commit updates `docs/adr/` in the same commit: a new ADR for a new decision, an edit for a
  changed decision, and the README for new measurements or open questions.

**Why.** Authorship should reflect the team. Keeping the ADRs in step with each commit means the
records explain every version that gets submitted and scored.

**Consequences.** The hook lives in `.git/hooks/` and is not pushed, so a fresh clone needs it
reinstalled. Commits made before this ADR still carry the Claude trailer unless history is
rewritten.
