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

## Benchmark machine (AWS), the source of truth for speed

Timings on the owner's Mac are NOT representative (ARM CPU, 16 KB pages, memory pressure). Measure on
the AWS VM instead:
- **Instance:** `m5.2xlarge` in us-east-1: Intel Xeon Platinum 8175M (Skylake-SP, 2.5 GHz), 4 cores × 2 HT =
  8 vCPU, L2 1 MB/core, L3 33 MB, AVX2 + AVX-512, THP = `madvise`, 32 GB RAM, Ubuntu 24.04, Rust 1.85.
  Launched with a 3-hour self-delete (`shutdown -h +180`, terminate on shutdown).
- **Access:** key pair `hackathon-aws` (private key only at `~/.ssh/hackathon-aws` on the Mac, never
  uploaded), security group `hackathon-ssh` (SSH from the owner's IP only).
- **On the VM:** repo at `~/ss`, generator at `~/generator`, official answer at `~/dev-answer`, Python
  venv at `~/.venv`. Datasets: `dev` (1M), `dev20` (20M, practice size), `dev60` (60M, final size).
- **Workflow:** rsync `src`, `Cargo.*`, `run.sh` and `scripts` to `~/ss`, run `scripts/check.sh full` there with
  `RUSTFLAGS="-C target-cpu=x86-64-v3"`, then time 5 alternating runs against the current best at 20M,
  with `run.sh`'s environment.
- **Calibration:** the VM has been about 15–20% slower than the scorer (VM 2.40 s → scorer 2.01 s).

## Where to run things

**Do not run heavy work on the owner's laptop** (8 GB Mac, often near its memory limit): no
benchmarks, test suites, dataset generation, Docker, or large builds there. Run them on a cloud
Linux VM (AWS or GCP, ideally matching the scorer: x86 Intel, 8 vCPUs; GCP n2-standard-8 or AWS
c6i.2xlarge), created with the owner's approval and deleted or stopped when done.
Light work is fine locally: reading and editing code, git, small scripts.
The project venv for Python is at `../.venv`.
