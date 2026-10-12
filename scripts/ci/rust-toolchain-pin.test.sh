#!/usr/bin/env bash
# Guard: one pinned rustc everywhere (rust-toolchain.toml, CI images, workflow
# toolchain installs) and the PR CI path runs `cargo fmt --check` + clippy -D warnings.
# RUST_PIN_ROOT overrides the repo root (used to probe the guard on a temp copy).
set -euo pipefail

ROOT="${RUST_PIN_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
cd "$ROOT"

fail() { echo "FAIL: $*" >&2; exit 1; }

[[ -f rust-toolchain.toml ]] || fail "rust-toolchain.toml missing"
channel="$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' rust-toolchain.toml | head -1)"
[[ "$channel" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "rust-toolchain.toml channel must be an exact X.Y.Z version (got '${channel}')"
grep -Eq '^components *=.*"rustfmt"' rust-toolchain.toml || fail "rust-toolchain.toml must list rustfmt"
grep -Eq '^components *=.*"clippy"' rust-toolchain.toml || fail "rust-toolchain.toml must list clippy"
echo "ok: rust-toolchain.toml channel ${channel}"

# CI images: glibc Dockerfile installs rustup directly; the Alpine image calls the install script.
for f in docker/ci/Dockerfile scripts/ci/install-alpine-native-toolchain.sh; do
  v="$(sed -n 's/.*--default-toolchain \([^ ]*\).*/\1/p' "$f" | head -1)"
  [[ -n "$v" ]] || fail "$f: no --default-toolchain found"
  [[ "$v" == "$channel" ]] || fail "$f: --default-toolchain ${v} != rust-toolchain.toml ${channel}"
done
if grep -rn -- '--default-toolchain stable' docker scripts .github 2>/dev/null | grep -v 'rust-toolchain-pin.test.sh'; then
  fail "--default-toolchain stable remains"
fi
echo "ok: CI images install ${channel}"

# Workflow / composite-action toolchain installs.
refs="$(grep -rhoE 'dtolnay/rust-toolchain@[A-Za-z0-9._-]+' .github | sort -u || true)"
[[ -n "$refs" ]] || fail "no dtolnay/rust-toolchain usage found under .github"
while IFS= read -r r; do
  [[ "$r" == "dtolnay/rust-toolchain@${channel}" ]] || fail "${r} != dtolnay/rust-toolchain@${channel}"
done <<<"$refs"
if grep -rnE 'toolchain: *(stable|nightly|beta)' .github; then
  fail "workflow sets a floating toolchain input"
fi
echo "ok: dtolnay/rust-toolchain pinned to ${channel}"

# Lint path in PR CI.
lint=scripts/ci/run-rust-lint.sh
[[ -f "$lint" ]] || fail "$lint missing"
grep -q 'cargo fmt --all -- --check' "$lint" || fail "$lint must run 'cargo fmt --all -- --check'"
grep -Eq 'cargo clippy .*-- -D warnings' "$lint" || fail "$lint must run 'cargo clippy ... -- -D warnings'"
grep -q 'run-rust-lint.sh' scripts/ci/run-pr-integration.sh || fail "run-pr-integration.sh must call run-rust-lint.sh"
echo "ok: PR CI runs cargo fmt --check and clippy -D warnings"
