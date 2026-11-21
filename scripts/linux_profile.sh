#!/bin/sh
# Profile the pipeline on Linux (inside Docker) with perf (ADR-021).
#   scripts/linux_profile.sh [DATA_DIR] [EXTRA_ENV...]    default DATA_DIR: dev5
# Example: scripts/linux_profile.sh dev5 MIMALLOC_ALLOW_LARGE_OS_PAGES=1
# Needs Docker running. The container is privileged only so perf may read
# hardware counters; it mounts the data read-only and writes to a temp dir.
set -eu
cd "$(dirname "$0")/.."
DATA="${1:-dev5}"; [ $# -gt 0 ] && shift
ENVS=""
for e in "$@"; do ENVS="$ENVS -e $e"; done
docker build -q -f Dockerfile.profile -t ss-profile . >/dev/null
# shellcheck disable=SC2086
docker run --rm --privileged --network none \
  -v "$PWD/$DATA":/data_host:ro -e DATA_DIR=/data -e OUTPUT_DIR=/tmp/out -e NCPU=8 $ENVS \
  ss-profile sh -c '
    PERF=perf
    # Copy the data into the VM first: Docker on macOS serves bind mounts through a
    # slow file-sharing bridge, which would dominate the timings.
    cp -r /data_host /data
    sysctl -qw kernel.perf_event_paranoid=-1 2>/dev/null || true
    BIN=/src/target/release/shopping-sessions
    echo "== warm-up run (fills the page cache)"; $BIN 2>&1 | tail -1; rm -rf /tmp/out
    echo "== stage timings"; $BIN 2>&1; rm -rf /tmp/out
    echo "== perf stat"
    $PERF stat -e task-clock,page-faults,minor-faults,major-faults,context-switches,cpu-migrations $BIN 2>&1 | grep -vE "^\[|^    (events|links|clicks)|^$" ; rm -rf /tmp/out
    echo "== perf record (top functions, self time)"
    $PERF record -q -g -o /tmp/perf.data $BIN >/dev/null 2>&1; rm -rf /tmp/out
    $PERF report -i /tmp/perf.data --stdio --no-children --percent-limit 1.5 --sort symbol -g none 2>/dev/null | grep -vE "^#|^$" | head -40
  '
