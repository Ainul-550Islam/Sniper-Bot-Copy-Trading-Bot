# ============================================================================
# sniper-suite — multi-stage build for the control-plane binary.
#
# Builds the `sniper-suite` server (Axum REST + WebSocket + dashboard) and all
# trading modules in a Rust builder stage, then ships a slim runtime image.
#
# Module 4 (programs/staking-suite) is an on-chain BPF program; it is excluded
# from the workspace and is built separately with the Solana toolchain
# (cargo build-sbf). See README "Deploying the staking program".
#
# Build:   docker build -t sniper-suite .
# Run:     docker run --rm -p 8080:8080 --env-file .env \
#            -v $PWD/config.toml:/app/config.toml:ro \
#            -v $PWD/data:/app/data sniper-suite
# ============================================================================

# ---- builder ----------------------------------------------------------------
# Toolchain matches rust-toolchain.toml (1.98.1 — the version this codebase is
# verified against; the file is copied below so rustup sees the same pin and
# the image, CI and local builds cannot drift apart).
# Pinned BY DIGEST, not by tag (P0 §7). `rust:1.98.1-bookworm` is a mutable
# pointer — the same tag rebuilds with new base packages, so two builds of
# the "same" Dockerfile are not the same image and a rollback does not
# necessarily roll back. The digest IS the image.
# Refresh with ./scripts/pin-base-image-digests.sh (keeps
# deploy/release/base-images.lock.json authoritative).
# rust:1.98.1-bookworm
FROM rust@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder

# Solana/reqwest builds want these native tools present.
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libudev-dev \
        protobuf-compiler \
        cmake \
        build-essential \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests + lockfile + toolchain pin first for better layer caching,
# then the sources.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates

# The release profile enables LTO/overflow-checks (see root Cargo.toml).
# `--offline` is intentionally NOT used: the builder fetches crates normally.
RUN cargo build --release --bin sniper-suite

# ---- runtime ----------------------------------------------------------------
# debian:bookworm-slim — digest-pinned, see the note on the builder stage.
FROM debian@sha256:913f6706df59a68922d1dd08f78c2476560a8d367897200a6005b00e5f67c2d5 AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libudev0 \
        curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 sniper

WORKDIR /app

COPY --from=builder /app/target/release/sniper-suite /usr/local/bin/sniper-suite
COPY config.toml.example /app/config.example.toml

# Persistent data (trades/positions/events JSONL) lives here; mount a volume.
RUN mkdir -p /app/data && chown -R sniper:sniper /app
USER sniper

# Control-plane API + dashboard.
EXPOSE 8080

ENV CONFIG_PATH=/app/config.toml \
    RUST_LOG=info

# Simple healthcheck against the control API.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1

ENTRYPOINT ["/usr/local/bin/sniper-suite"]
