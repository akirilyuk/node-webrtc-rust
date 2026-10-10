# Build and run image for scripts/perf/run-voice-cost.sh (speech engine cost probes).
# Pinned to the toolchain the published numbers were measured with; bump it deliberately,
# results are only comparable within one rust minor version.
FROM rust:1.99-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
