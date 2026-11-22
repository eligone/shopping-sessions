#!/bin/sh
# Every correctness check for the Rust pipeline, in one command. Run before each commit
# that touches src/. Stops at the first failure.
#   scripts/check.sh          unit tests, fixtures (both read modes), dev set, 2 diff seeds
#   scripts/check.sh full     ... plus diff seeds 2-10 and the 5M set (dev5/) against DuckDB
set -eu
cd "$(dirname "$0")/.."
PY=../.venv/bin/python
BIN=target/release/shopping-sessions

LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT
# Run a command quietly; on failure show its output and stop. (Piping into grep
# would hide the exit status: in `a | b` only b's status counts.)
step() {
  if ! "$@" > "$LOG" 2>&1; then
    tail -40 "$LOG"; echo "CHECK FAILED: $*"; exit 1
  fi
}

echo "== build"
step nice -n 10 cargo build --release -j 2
grep -E "^warning" "$LOG" || true
echo "ok"

echo "== unit tests"
step nice -n 10 cargo test --release -j 2
grep -E "test result" "$LOG"

for mode in pread mmap; do
  echo "== fixtures ($mode)"
  SS_READ=$mode step "$PY" scripts/run_fixtures.py fixtures -- "$BIN"
  tail -1 "$LOG"
done

echo "== dev set vs dev-answer"
rm -rf out_check
DATA_DIR=dev OUTPUT_DIR=out_check NCPU=4 step "$BIN"
(cd scripts && step "../$PY" compare.py ../out_check ../../dev-answer)
tail -1 "$LOG"
rm -rf out_check

if [ "${1:-}" = "full" ]; then
  echo "== diff vs DuckDB, seeds 2-10 + calendar edges 15 62 66 133 (1M events)"
  scripts/diff_test.sh 1000000 2 3 4 5 6 7 8 9 10 15 62 66 133
  if [ -d dev5 ]; then
    echo "== diff vs DuckDB, dev5 (5M events)"
    rm -rf o5_rs o5_db s5
    DATA_DIR=dev5 OUTPUT_DIR=o5_rs NCPU=4 step "$BIN"
    mkdir -p s5 && DATA_DIR=dev5 OUTPUT_DIR=o5_db SCRATCH_DIR=s5 NCPU=4 MEM_GB=3 step nice -n 10 "$PY" pipeline/pipeline.py
    (cd scripts && step "../$PY" compare.py ../o5_rs ../o5_db)
    tail -1 "$LOG"
    rm -rf o5_rs o5_db s5
  fi
else
  echo "== diff vs DuckDB, seeds 2-3 (1M events)"
  scripts/diff_test.sh 1000000 2 3
fi
echo "ALL CHECKS PASSED"
