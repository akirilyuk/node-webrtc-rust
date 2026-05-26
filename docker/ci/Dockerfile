# CI image for node-webrtc-rust — Node, Rust, Zig, and cross-compile tooling.
# Used by GitHub Actions for Linux native builds and the test job.

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    build-essential \
    pkg-config \
    python3 \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Node.js 20 LTS
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get update \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# Rust (stable) + Linux cross targets used in CI
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --profile minimal \
    && rustup target add \
      x86_64-unknown-linux-gnu \
      x86_64-unknown-linux-musl \
      aarch64-unknown-linux-gnu

# Zig (napi-rs cross-linking via `napi build --zig`)
# Tarball names changed in 0.14.1 — resolve URL from the official index.
ARG ZIG_VERSION=0.14.1
RUN set -eux \
    && ZIG_URL="$(curl -fsSL https://ziglang.org/download/index.json \
      | python3 -c "import json,sys; print(json.load(sys.stdin)['${ZIG_VERSION}']['x86_64-linux']['tarball'])")" \
    && curl -fsSL "$ZIG_URL" -o /tmp/zig.tar.xz \
    && ZIG_DIR="$(tar -tf /tmp/zig.tar.xz | head -1 | cut -f1 -d/)" \
    && tar -xJf /tmp/zig.tar.xz -C /usr/local \
    && ln -sf "/usr/local/${ZIG_DIR}/zig" /usr/local/bin/zig \
    && rm /tmp/zig.tar.xz \
    && zig version

WORKDIR /workspace
