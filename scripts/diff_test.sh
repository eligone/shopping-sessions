#!/bin/sh
# Differential test: the Rust binary and the DuckDB reference must agree exactly.
# Usage: scripts/diff_test.sh [EVENTS] [SEED...]      (default: 1000000 events, seeds 2-10)
# Needs: target/release/shopping-sessions, ../generator/target/release/generate, ../.venv with duckdb.
# Each seed's data is generated, checked and deleted, so disk use stays at one dataset.
set -eu
cd "$(dirname "$0")/.."
EVENTS="${1:-1000000}"
[ $# -gt 0 ] && shift
SEEDS="${*:-2 3 4 5 6 7 8 9 10}"
GEN=../generator/target/release/generate
PY=../.venv/bin/python
BIN=target/release/shopping-sessions
CLICKS=$((EVENTS / 4))
fail=0
for seed in $SEEDS; do
  d="dev_seed$seed"
  rm -rf "$d" out_rs_$seed out_db_$seed scratch_$seed
  nice -n 10 "$GEN" --seed "$seed" --events "$EVENTS" --clicks "$CLICKS" --output "$d" >/dev/null
  DATA_DIR="$d" OUTPUT_DIR="out_rs_$seed" NCPU=4 nice -n 10 "$BIN" 2>/dev/null
  mkdir -p "scratch_$seed"
  DATA_DIR="$d" OUTPUT_DIR="out_db_$seed" SCRATCH_DIR="scratch_$seed" NCPU=4 MEM_GB=3 \
    nice -n 10 "$PY" pipeline/pipeline.py >/dev/null 2>&1
  if result=$(cd scripts && "../$PY" compare.py "../out_rs_$seed" "../out_db_$seed"); then
    echo "seed $seed: MATCH"
  else
    echo "seed $seed: DIFFER"; echo "$result"; fail=1
  fi
  rm -rf "$d" out_rs_$seed out_db_$seed scratch_$seed
done
exit $fail
