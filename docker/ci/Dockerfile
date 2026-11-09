# CI image for node-webrtc-rust — Node, Rust, Zig, and cross-compile tooling.
# Used by GitHub Actions for Linux native builds and the test job.

FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    cmake \
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

# Rust (pinned 1.99.0, see rust-toolchain.toml) + Linux cross targets used in CI
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain 1.99.0 --profile minimal \
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

# sherpa-onnx-sys prebuilt static libs (glibc x64), baked so cargo builds never download them
# from GitHub releases at build time (a 504 on that URL failed every PR's rust lint, 2026-11-09).
# build.rs copies the archive from SHERPA_ONNX_ARCHIVE_DIR and errors with "does not contain
# expected archive" when the file for the Cargo.lock version is missing, so a sherpa-onnx-sys bump
# without an image bump fails loud instead of downloading.
# SHERPA_ONNX_VERSION must equal sherpa-onnx-sys in Cargo.lock: scripts/ci/check-sherpa-onnx-archive.sh
# --static (run-pr-quality.sh). The sha256 is the release asset digest GitHub publishes
# (gh api repos/k2-fsa/sherpa-onnx/releases/tags/v<ver> → assets[].digest).
# Download: the release URL first; if it fails, the same asset through the releases API.
ARG SHERPA_ONNX_VERSION=1.13.2
ARG SHERPA_ONNX_LINUX_X64_STATIC_SHA256=6a53f4e117853f0e5a2df75b33fde066972dc1bb920984b2674859ed003446ea
ENV SHERPA_ONNX_ARCHIVE_DIR=/opt/sherpa-onnx-archives
RUN set -eux \
    && name="sherpa-onnx-v${SHERPA_ONNX_VERSION}-linux-x64-static-lib.tar.bz2" \
    && out="${SHERPA_ONNX_ARCHIVE_DIR}/${name}" \
    && mkdir -p "${SHERPA_ONNX_ARCHIVE_DIR}" \
    && if ! curl -fsSL --retry 3 --retry-all-errors -o "$out" \
         "https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_ONNX_VERSION}/${name}"; then \
         asset_url="$(curl -fsSL --retry 3 --retry-all-errors \
           "https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/tags/v${SHERPA_ONNX_VERSION}" \
           | python3 -c "import json,sys; n=sys.argv[1]; print(next(a['url'] for a in json.load(sys.stdin)['assets'] if a['name'] == n))" "$name")"; \
         curl -fsSL --retry 3 --retry-all-errors -H 'Accept: application/octet-stream' -o "$out" "$asset_url"; \
       fi \
    && echo "${SHERPA_ONNX_LINUX_X64_STATIC_SHA256}  ${out}" | sha256sum -c - \
    && test -s "$out"

WORKDIR /workspace
