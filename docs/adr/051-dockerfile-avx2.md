# ADR-051: Build with AVX2 whatever the Docker builder (fix the TARGETARCH check)

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
The scorer tracked our VM gains weakly (VM 2.15 → 1.93 s, scorer 1.68 → 1.62 s). We built the image on
the VM and ran it with the scorer's exact `docker run` flags (`scripts/docker_run.sh`). The run took
**3.63 s**, against 1.93 s for the bare binary, and the events parse took 2.13 s against 0.95 s. Removing
`--cpus`, `--memory` or the security flags changed nothing.

The cause is in the Dockerfile. It sets `RUSTFLAGS=-C target-cpu=x86-64-v3` only when
`$TARGETARCH = amd64`, but only BuildKit sets `TARGETARCH`. With the classic builder (Ubuntu's
`docker.io` has no buildx), the variable is empty. The binary is then built for baseline x86-64 with
no AVX2, so `block_masks` falls back to its byte loop and every other AVX2-dependent path slows down.
If the scorer builds this way, none of our SIMD work ever reached it.

## Decision
Also enable x86-64-v3 when `uname -m` is `x86_64`, so the build machine's real architecture decides.
The scorer is Cascade Lake or newer, so it has AVX2. An arm64 build still gets the portable code.

## Measurements (VM, 20M, image run with the scorer's docker flags, 3 runs)
| Image | Container wall | Program TOTAL | Events parse |
|---|---|---|---|
| Old Dockerfile (no AVX2 under the classic builder) | 3.61-3.64 s | 3.28-3.32 s | 2.13 s |
| ADR-051 | 2.25 s | 1.93 s | 0.95 s |
An empty container costs 0.23 s on this VM. The code is `73af7c2` (ADR-050's experiment reverted).
