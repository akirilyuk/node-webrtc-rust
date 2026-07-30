#!/usr/bin/env bash
# Argument and environment contract tests for build-native-addon.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/ci/build-native-addon.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/build-native-addon-XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

mkdir -p "$TMP/bin" "$TMP/bindings"
cat >"$TMP/bin/npx" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >"$NATIVE_BUILD_TEST_NPX"
printf 'CMAKE=%s\nOPUS=%s\nSHERPA=%s\nLD=%s\n' \
  "${CMAKE_POLICY_VERSION_MINIMUM:-}" \
  "${OPUS_STATIC:-}" \
  "${SHERPA_ONNX_LIB_DIR:-}" \
  "${LD_LIBRARY_PATH:-}" >>"$NATIVE_BUILD_TEST_NPX"
EOF
cat >"$TMP/bin/npm" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >"$NATIVE_BUILD_TEST_NPM"
EOF
chmod +x "$TMP/bin/npx" "$TMP/bin/npm" "$SCRIPT"

run_recipe() {
  PATH="$TMP/bin:$PATH" \
    NATIVE_BUILD_BINDINGS_DIR="$TMP/bindings" \
    NATIVE_BUILD_TEST_NPX="$TMP/npx.out" \
    NATIVE_BUILD_TEST_NPM="$TMP/npm.out" \
    bash "$SCRIPT" "$@"
}

echo "==> Linux debug recipe"
rm -f "$TMP/npx.out" "$TMP/npm.out"
CMAKE_POLICY_VERSION_MINIMUM=runner-value OPUS_STATIC=runner-value run_recipe \
  --target x86_64-unknown-linux-gnu \
  --profile debug
rg -q '^napi build --target x86_64-unknown-linux-gnu$' "$TMP/npx.out"
rg -q '^run copy:local-node$' "$TMP/npm.out"
rg -q '^CMAKE=3.5$' "$TMP/npx.out"
rg -q '^OPUS=1$' "$TMP/npx.out"
echo "ok: Linux debug args + local copy"

echo "==> Linux release platform recipe"
rm -f "$TMP/npx.out" "$TMP/npm.out"
run_recipe \
  --target x86_64-unknown-linux-musl \
  --profile release
rg -q '^napi build --target x86_64-unknown-linux-musl --release --features otel --platform$' "$TMP/npx.out"
rg -q '^SHERPA=/opt/sherpa-musl/lib$' "$TMP/npx.out"
test ! -e "$TMP/npm.out"
echo "ok: Linux release args + Sherpa environment"

echo "==> Host release recipe"
rm -f "$TMP/npx.out" "$TMP/npm.out"
run_recipe \
  --target x86_64-apple-darwin \
  --profile release
rg -q '^napi build --target x86_64-apple-darwin --release --features otel --platform$' "$TMP/npx.out"
test ! -e "$TMP/npm.out"
echo "ok: host args preserve matrix build arguments"

echo "build-native-addon.test.sh: all checks passed"
