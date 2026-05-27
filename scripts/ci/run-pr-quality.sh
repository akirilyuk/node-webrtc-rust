#!/usr/bin/env bash
# Fast TS quality gates — no native .node required (workspace npm ci only).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> npm ci"
npm ci

echo "==> typecheck (packages only — examples need build:ts first)"
npm run typecheck --workspace=@node-webrtc-rust/sdk --workspace=@node-webrtc-rust/signaling

echo "==> lint"
npm run lint

echo "==> Quality checks OK"
