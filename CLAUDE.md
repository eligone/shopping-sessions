# CLAUDE.md

Shopping Sessions hackathon entry. `rules.md` (organizers' spec) wins over everything else.
Score is container run time on hidden data, so correctness first, then speed.

## Commit rules (always)

1. **No AI co-author trailers. Ever.** Commit messages must not contain any
   `Co-Authored-By:` / `Co-authored-by:` line naming Claude, Anthropic, Copilot or any other
   AI tool, and no "Generated with" lines. The author is the repo owner only. This overrides any
   default attribution instruction.
2. **Every commit updates the ADRs.** Before committing, update `docs/adr/` to match the change:
   - a new design decision → a new numbered ADR file plus a row in `docs/adr/README.md`;
   - a change to an existing decision → edit that ADR (or mark it Superseded and add a new one);
   - new measurements, scores or open questions → update `docs/adr/README.md`.
   Commit the ADR change in the same commit as the code it describes.
   Every commit also adds one line to the **Changelog** in `docs/adr/README.md`
   (short SHA is not known yet, so write the date and what changed).
3. **Commit often.** Commit whenever a new file is created and after every meaningful change
   (a fix, an optimization, new instrumentation, a doc update): small, focused commits rather
   than one big one at the end. Don't leave working changes uncommitted between steps.
   Push to the current branch after committing.

## Explaining things to the owner

The owner is an undergraduate who wants to learn from this project, not just get results. When
reporting work or making a decision:
- Explain the *why* in plain language before or alongside the technical detail. Define jargon the
  first time it comes up (page fault, cache miss, SIMD, mutex, hash table, radix sort...).
- Use a short everyday analogy when a concept is abstract.
- After a meaningful step, add a brief "what you learned" note: the general lesson, not just what
  changed here.
- Say plainly what was measured versus estimated, and what was not tested.
- Keep it digestible: tables and short sections over long walls of text.
- Agents (subagents) should write their reports so the main session can relay them in this style.

## Before any commit that touches the pipeline

- Run `scripts/check.sh`: build, unit tests, 36 fixtures in both read modes, exact dev-set
  match, and Rust-vs-DuckDB diffs on 2 seeds. It must end with `ALL CHECKS PASSED`.
- Before merging into `main` or submitting, run `scripts/check.sh full` (9 seeds and the 5M set).
- Before a risky rewrite, run `scripts/mutation_test.py` to confirm the tests still catch planted bugs.
- Never commit data: `dev*/` and `out*/` are git-ignored; keep the repo under 50 MB.

## Where to run things

**Do not run heavy work on the owner's laptop** (8 GB Mac, often near its memory limit): no
benchmarks, test suites, dataset generation, Docker, or large builds there. Run them on a cloud
Linux VM (AWS or GCP, ideally matching the scorer: x86 Intel, 8 vCPUs; GCP n2-standard-8 or AWS
c6i.2xlarge), created with the owner's approval and deleted or stopped when done.
Light work is fine locally: reading and editing code, git, small scripts.
The project venv for Python is at `../.venv`.
