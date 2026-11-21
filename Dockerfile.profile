# Profiling image (NOT the submission image; see Dockerfile for that).
# Builds the pipeline inside Linux with perf installed and debug line info kept,
# so `perf` can attribute time to functions and source lines (ADR-021).
FROM rust:1.85-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends linux-perf && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ENV CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
RUN cargo build --release --locked
