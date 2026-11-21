#!/bin/sh
# Entry point. Runs with no arguments; exit 0 means "done, check my output".
set -eu
: "${DATA_DIR:=/data}" "${OUTPUT_DIR:=/output}" "${SCRATCH_DIR:=/scratch}"
export DATA_DIR OUTPUT_DIR SCRATCH_DIR
export TMPDIR="$SCRATCH_DIR" HOME="$SCRATCH_DIR"
# Ask mimalloc for 2 MB OS pages: far fewer page faults and TLB misses on a
# kernel where transparent huge pages are only given on request (ADR-022).
# mimalloc falls back to normal pages if huge pages are unavailable.
export MIMALLOC_ALLOW_LARGE_OS_PAGES=1
exec /app/shopping-sessions
