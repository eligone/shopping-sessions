#!/usr/bin/env python3
"""
Mutation test: plant one realistic bug at a time and check that our tests catch it.

For each mutation: apply it, rebuild, run the checks cheapest-first (unit tests,
fixtures, dev set, one Rust-vs-DuckDB seed), record which check failed first,
then restore the file. A mutation that no check catches has "survived": either
a gap in the tests, or an equivalent mutant (a change that cannot alter output).

Usage: ../.venv/bin/python scripts/mutation_test.py     (from starter-duckdb/)
"""
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PY = os.path.join(ROOT, "..", ".venv", "bin", "python")
BIN = os.path.join(ROOT, "target", "release", "shopping-sessions")

# (file, original text, mutated text, what the bug means)
MUTATIONS = [
    ("src/events.rs", "ingest.saturating_sub(ts) <= ts::MS_PER_DAY", "ingest.saturating_sub(ts) < ts::MS_PER_DAY",
     "late cutoff drops events exactly 24 h late"),
    ("src/events.rs", "&& ts >= ws", "&& ts > ws", "event window excludes its start"),
    ("src/events.rs", "if r.ingest < cur.ingest {", "if r.ingest > cur.ingest {", "dedup keeps the LATEST copy"),
    ("src/events.rs", "(1..=MAX_PRICE as u64)", "(0..=MAX_PRICE as u64)", "price 0 accepted"),
    ("src/events.rs", 'Some(b"purchase") => PURCHASE', 'Some(b"purchase") | Some(b"Purchase") => PURCHASE',
     "event type case-insensitive"),
    ("src/sessions.rs", "ev[j].ts - ev[j - 1].ts <= GAP", "ev[j].ts - ev[j - 1].ts < GAP",
     "a gap of exactly 30 min splits the session"),
    ("src/sessions.rs", "top.is_some_and(|t| c < t)", "top.is_some_and(|t| c > t)",
     "top_category tie goes to the LARGEST name"),
    ("src/attribution.rs", "s.start - pc[group].ts <= WEEK", "s.start - pc[group].ts < WEEK",
     "a click exactly 7 days before does not credit"),
    ("src/attribution.rs", "by_camp[k].1 - last <= WEEK", "by_camp[k].1 - last < WEEK",
     "click ranges touching at one instant are not merged (expected EQUIVALENT)"),
    ("src/clicks.rs", "if t < lo || t >= hi {", "if t < lo || t > hi {", "a click at window_end is kept"),
    ("src/links.rs", "ids::cmp_ids(keys[i], name[r]).is_lt()", "ids::cmp_ids(keys[i], name[r]).is_gt()",
     "person named by the LARGEST ID"),
    ("src/ts.rs", "{ t - off } else { t + off }", "{ t + off } else { t - off }", "offset sign flipped"),
    ("src/ts.rs", "d > days_in_month(y, mo)", "d > 31", "February 30 accepted"),
    ("src/ts.rs", "if oh > 23 || om > 59 {", "if oh > 24 || om > 59 {", "offset +24:00 accepted"),
    ("src/csv.rs", "if e - s < 2 || fq != ", "if e - s < 1 || fq != ", "a lone quote is a valid field"),
]


def run(cmd, env=None):
    return subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)


def checks():
    """Return the name of the first failing check, or None if all pass."""
    if run(["cargo", "build", "--release", "-q"]).returncode != 0:
        return "build"
    if run(["cargo", "test", "--release", "-q"]).returncode != 0:
        return "unit tests"
    if run([PY, "scripts/run_fixtures.py", "fixtures", "--", BIN]).returncode != 0:
        return "fixtures"
    with tempfile.TemporaryDirectory() as out:
        env = dict(os.environ, DATA_DIR=os.path.join(ROOT, "dev"), OUTPUT_DIR=out, NCPU="4")
        if run([BIN], env=env).returncode != 0:
            return "dev set (crash)"
        cmp = subprocess.run([PY, "compare.py", out, os.path.join(ROOT, "..", "dev-answer")],
                             cwd=os.path.join(ROOT, "scripts"), capture_output=True)
        if cmp.returncode != 0:
            return "dev set"
    if run(["sh", "scripts/diff_test.sh", "1000000", "2"]).returncode != 0:
        return "diff vs DuckDB"
    return None


def main():
    results = []
    for path, old, new, meaning in MUTATIONS:
        full = os.path.join(ROOT, path)
        src = open(full).read()
        if src.count(old) != 1:
            sys.exit(f"mutation target not found exactly once in {path}: {old!r}")
        backup = full + ".mutbak"
        shutil.copy(full, backup)
        try:
            open(full, "w").write(src.replace(old, new))
            caught = checks()
        finally:
            shutil.move(backup, full)
        status = f"CAUGHT by {caught}" if caught else "SURVIVED"
        print(f"{status:<28} {path:<20} {meaning}", flush=True)
        results.append(caught)
    run(["cargo", "build", "--release", "-q"])  # rebuild the unmutated binary
    caught = sum(1 for r in results if r)
    print(f"\n{caught}/{len(results)} mutations caught")


if __name__ == "__main__":
    main()
