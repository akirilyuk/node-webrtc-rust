#!/usr/bin/env bash
# Canonical release native targets (napi-rs triples) — single source of truth for the six
# published platform packages. Used by plan/build/stage scripts, native fingerprint /
# provenance manifests, and check-release-targets.sh. Do not add triples here unless a
# matching packages/bindings/npm/<dir> + optionalDependency ships in the same change.
# Loader fallbacks in packages/bindings/index.js may mention additional platforms; those
# are not release targets and must not be planned from this list.
set -euo pipefail

cat <<'EOF'
x86_64-unknown-linux-gnu
x86_64-unknown-linux-musl
aarch64-unknown-linux-gnu
x86_64-apple-darwin
aarch64-apple-darwin
x86_64-pc-windows-msvc
EOF
