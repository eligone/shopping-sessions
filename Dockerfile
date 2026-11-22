# Shopping Sessions: Rust pipeline. Multi-stage: compile, then ship only the binary.
# The scorers are linux/amd64 (Intel Xeon, Cascade Lake or newer): on amd64 the
# build targets x86-64-v3 (AVX2). No FROM line pins a platform, so the image
# also builds natively on arm64 for local testing.
FROM rust:1.85-bookworm AS build
ARG TARGETARCH
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Decide by the build machine's real architecture, not $TARGETARCH: only BuildKit
# sets TARGETARCH, and without it the old check silently built a binary with no
# AVX2, so the SIMD scanner fell back to its byte loop (ADR-051).
RUN if [ "$TARGETARCH" = "amd64" ] || [ "$(uname -m)" = "x86_64" ]; then export RUSTFLAGS="-C target-cpu=x86-64-v3"; fi; \
    cargo build --release --locked && strip target/release/shopping-sessions

FROM debian:bookworm-slim
# The scorer runs the container as uid 1000; give that uid a name and a home.
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app
COPY --from=build /src/target/release/shopping-sessions /app/shopping-sessions
COPY run.sh /app/run.sh
RUN chmod +x /app/run.sh /app/shopping-sessions
WORKDIR /app
CMD ["/app/run.sh"]
