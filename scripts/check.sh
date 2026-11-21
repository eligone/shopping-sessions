#!/bin/sh
# Every correctness check for the Rust pipeline, in one command. Run before each commit
# that touches src/. Stops at the first failure.
#   scripts/check.sh          unit tests, fixtures (both read modes), dev set, 2 diff seeds
#   scripts/check.sh full     ... plus diff seeds 2-10 and the 5M set (dev5/) against DuckDB
set -eu
cd "$(dirname "$0")/.."
PY=../.venv/bin/python
BIN=target/release/shopping-sessions

echo "== build"
nice -n 10 cargo build --release -j 4 2>&1 | grep -E "^(warning|error)|Finished"

echo "== unit tests"
nice -n 10 cargo test --release -j 4 2>&1 | grep -E "test result|FAILED|panicked"

for mode in pread mmap; do
  echo "== fixtures ($mode)"
  SS_READ=$mode "$PY" scripts/run_fixtures.py fixtures -- "$BIN" > /tmp/ss_fixtures.$$ 2>&1 || { cat /tmp/ss_fixtures.$$; exit 1; }
  tail -1 /tmp/ss_fixtures.$$
done
rm -f /tmp/ss_fixtures.$$

echo "== dev set vs dev-answer"
rm -rf out_check
DATA_DIR=dev OUTPUT_DIR=out_check NCPU=4 "$BIN" 2>/dev/null
(cd scripts && "../$PY" compare.py ../out_check ../../dev-answer | tail -1) | grep -q MATCH || { echo "dev DIFFER"; exit 1; }
echo "MATCH"
rm -rf out_check

if [ "${1:-}" = "full" ]; then
  echo "== diff vs DuckDB, seeds 2-10 (1M events)"
  scripts/diff_test.sh 1000000 2 3 4 5 6 7 8 9 10
  if [ -d dev5 ]; then
    echo "== diff vs DuckDB, dev5 (5M events)"
    rm -rf o5_rs o5_db s5
    DATA_DIR=dev5 OUTPUT_DIR=o5_rs NCPU=4 "$BIN" 2>/dev/null
    mkdir -p s5 && DATA_DIR=dev5 OUTPUT_DIR=o5_db SCRATCH_DIR=s5 NCPU=4 MEM_GB=3 nice -n 10 "$PY" pipeline/pipeline.py >/dev/null 2>&1
    (cd scripts && "../$PY" compare.py ../o5_rs ../o5_db | tail -1)
    rm -rf o5_rs o5_db s5
  fi
else
  echo "== diff vs DuckDB, seeds 2-3 (1M events)"
  scripts/diff_test.sh 1000000 2 3
fi
echo "ALL CHECKS PASSED"
