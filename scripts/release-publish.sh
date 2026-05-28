#!/usr/bin/env bash
# Publish all npm packages from macOS (no Docker).
#
# Uses .node files already in packages/bindings/ (no rebuild if all 6 are present).
# Also checks packages/bindings/artifacts/ and packages/bindings/prebuilt/.
#
# 1. Paste NPM_TOKEN below (do not commit a real token).
# 2. ./scripts/release-publish.sh 0.1.1 [--otp=123456] [--dry-run] [--force-build]
#
set -euo pipefail

# --- authenticate via NPM_TOKEN env var or `npm login` (never commit tokens) ---
NPM_TOKEN="${NPM_TOKEN:-}"

# ---------------------------------------------------------------------------

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINDINGS="$ROOT/packages/bindings"
PREBUILT="$BINDINGS/prebuilt"
ARTIFACTS="$BINDINGS/artifacts"
VERSION="${1:-}"
DRY_RUN=false
FORCE_BUILD=false
NPM_OTP="${NPM_OTP:-}"

LINUX_TARGETS=(
  x86_64-unknown-linux-gnu
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-gnu
)
MACOS_TARGETS=(
  aarch64-apple-darwin
  x86_64-apple-darwin
)
WINDOWS_TARGET=x86_64-pc-windows-msvc
ALL_TARGETS=("${LINUX_TARGETS[@]}" "${MACOS_TARGETS[@]}" "$WINDOWS_TARGET")

shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true ;;
    --force-build) FORCE_BUILD=true ;;
    --otp=*) NPM_OTP="${1#*=}" ;;
    --otp)
      shift
      NPM_OTP="${1:-}"
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
  shift
done

if [[ -z "$VERSION" ]]; then
  echo "Usage: $0 <version> [--otp=CODE] [--dry-run] [--force-build]" >&2
  echo "  Or: NPM_OTP=123456 $0 <version>" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script expects macOS to cross-compile Linux and Darwin targets." >&2
  exit 1
fi

check_npm_auth() {
  if [[ -n "$NPM_TOKEN" ]]; then
    export NPM_TOKEN
  fi
  if ! npm whoami &>/dev/null; then
    echo "Not logged in to npm." >&2
    echo "  Run: npm login" >&2
    echo "  Or: export NPM_TOKEN=...  (see scripts/RELEASE.md)" >&2
    exit 1
  fi
  echo "==> npm user: $(npm whoami)"
  echo "  See scripts/RELEASE.md for one-time npm org setup"
}

check_npm_auth

export CMAKE_POLICY_VERSION_MINIMUM=3.5
export OPUS_STATIC=1
export CARGO_INCREMENTAL=0

require_cmd() {
  command -v "$1" &>/dev/null || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

has_node_in_dir() {
  local dir="$1"
  [[ -d "$dir" ]] && compgen -G "$dir"/*.node &>/dev/null
}

is_known_target() {
  local t="$1"
  local known
  for known in "${ALL_TARGETS[@]}"; do
    [[ "$known" == "$t" ]] && return 0
  done
  return 1
}

# Map napi-rs output filename -> Rust triple folder name.
napi_node_to_target() {
  local name="$1"
  local suffix="${name#node-webrtc-rust.}"
  suffix="${suffix%.node}"

  if is_known_target "$suffix"; then
    echo "$suffix"
    return 0
  fi

  case "$suffix" in
    linux-x64-gnu) echo x86_64-unknown-linux-gnu ;;
    linux-x64-musl) echo x86_64-unknown-linux-musl ;;
    linux-arm64-gnu) echo aarch64-unknown-linux-gnu ;;
    darwin-x64) echo x86_64-apple-darwin ;;
    darwin-arm64) echo aarch64-apple-darwin ;;
    win32-x64-msvc) echo x86_64-pc-windows-msvc ;;
    *) return 1 ;;
  esac
}

stage_node_file() {
  local target="$1"
  local src="$2"
  local label="$3"
  local dest="$ARTIFACTS/bindings-$target"
  mkdir -p "$dest"
  cp -f "$src" "$dest/"
  echo "  skip build ($label): $(basename "$src") -> bindings-$target"
}

# Pick up packages/bindings/*.node sitting next to index.js.
collect_from_bindings_folder() {
  [[ "$FORCE_BUILD" == true ]] && return 0

  echo "==> Scan packages/bindings/*.node"
  shopt -s nullglob
  local f name target
  for f in "$BINDINGS"/*.node; do
    name=$(basename "$f")
    if ! target=$(napi_node_to_target "$name"); then
      echo "  ignore: $name (unknown platform suffix)"
      continue
    fi
    stage_node_file "$target" "$f" "bindings/"
  done
}

ensure_artifact() {
  local target="$1"
  local pre="$PREBUILT/bindings-$target"
  local dest="$ARTIFACTS/bindings-$target"

  if [[ "$FORCE_BUILD" == true ]]; then
    return 1
  fi

  if has_node_in_dir "$dest"; then
    echo "  skip build (artifacts): bindings-$target"
    return 0
  fi

  if has_node_in_dir "$pre"; then
    mkdir -p "$dest"
    cp -f "$pre"/*.node "$dest/"
    echo "  skip build (prebuilt): bindings-$target"
    return 0
  fi

  return 1
}

stage_build_output() {
  local target="$1"
  local dest="$ARTIFACTS/bindings-$target"
  mkdir -p "$dest"
  shopt -s nullglob
  local files=("$BINDINGS"/*.node)
  if [[ ${#files[@]} -eq 0 ]]; then
    echo "No .node produced for $target" >&2
    exit 1
  fi
  cp -f "${files[@]}" "$dest/"
  rm -f "$BINDINGS"/*.node
  echo "  built bindings-$target"
}

build_linux_target() {
  local target="$1"
  (cd "$BINDINGS" && npx napi build --platform --release --target "$target" --zig)
  stage_build_output "$target"
}

build_macos_target() {
  local target="$1"
  (cd "$BINDINGS" && npx napi build --platform --release --target "$target")
  stage_build_output "$target"
}

echo "==> Release $VERSION"
mkdir -p "$ARTIFACTS" "$PREBUILT"

cd "$ROOT"

collect_from_bindings_folder

NEED_BINDINGS_CI=false
for target in "${LINUX_TARGETS[@]}" "${MACOS_TARGETS[@]}"; do
  if ! ensure_artifact "$target"; then
    NEED_BINDINGS_CI=true
  fi
done

if ! ensure_artifact "$WINDOWS_TARGET"; then
  echo "" >&2
  echo "Missing Windows .node. Expected in packages/bindings/ as:" >&2
  echo "  node-webrtc-rust.win32-x64-msvc.node" >&2
  echo "or under prebuilt/bindings-x86_64-pc-windows-msvc/" >&2
  exit 1
fi

if [[ "$NEED_BINDINGS_CI" == true ]]; then
  echo "==> Checking build prerequisites (cmake, zig, rust)"
  require_cmd cmake
  require_cmd zig
  require_cmd rustc
  require_cmd cargo
  require_cmd npm
  require_cmd npx
  rustup target add \
    x86_64-unknown-linux-gnu \
    x86_64-unknown-linux-musl \
    aarch64-unknown-linux-gnu \
    x86_64-apple-darwin \
    aarch64-apple-darwin

  cd "$BINDINGS"
  npm ci --ignore-scripts

  echo "==> Build Linux targets (Zig cross-compile)"
  for target in "${LINUX_TARGETS[@]}"; do
    ensure_artifact "$target" && continue
    echo "  $target"
    build_linux_target "$target"
  done

  echo "==> Build macOS targets"
  for target in "${MACOS_TARGETS[@]}"; do
    ensure_artifact "$target" && continue
    echo "  $target"
    build_macos_target "$target"
  done
else
  echo "==> All targets present — skipping compile"
fi

echo "==> Verify artifacts"
MISSING=()
for target in "${ALL_TARGETS[@]}"; do
  if ! has_node_in_dir "$ARTIFACTS/bindings-$target"; then
    MISSING+=("bindings-$target")
  fi
done
if [[ ${#MISSING[@]} -gt 0 ]]; then
  echo "Missing .node for:" >&2
  printf '  %s\n' "${MISSING[@]}" >&2
  echo "Add files under packages/bindings/ (see index.js naming) or run --force-build" >&2
  exit 1
fi

cd "$ROOT"
echo "==> napi artifacts"
cd "$BINDINGS"
npx napi artifacts --dir artifacts

sync_bindings_optional_deps() {
  local opt
  for opt in \
    bindings-darwin-arm64 \
    bindings-darwin-x64 \
    bindings-linux-x64-gnu \
    bindings-linux-x64-musl \
    bindings-linux-arm64-gnu \
    bindings-win32-x64-msvc; do
    npm pkg set "optionalDependencies.@node-webrtc-rust/${opt}=${VERSION}" \
      --workspace=@node-webrtc-rust/bindings
  done
}

echo "==> Set version $VERSION"
cd "$BINDINGS"
npm version "$VERSION" --no-git-tag-version --allow-same-version
# --allow-same-version skips the version lifecycle; run napi version explicitly.
if git rev-parse --is-inside-work-tree &>/dev/null; then
  git config --global --add safe.directory "$ROOT" 2>/dev/null || true
fi
npx napi version
cd "$ROOT"
sync_bindings_optional_deps
npm version "$VERSION" --no-git-tag-version --allow-same-version --workspace=@node-webrtc-rust/sdk
npm version "$VERSION" --no-git-tag-version --allow-same-version --workspace=@node-webrtc-rust/signaling
npm pkg set "dependencies.@node-webrtc-rust/bindings=${VERSION}" --workspace=@node-webrtc-rust/sdk
npm pkg set "dependencies.@node-webrtc-rust/signaling=${VERSION}" --workspace=@node-webrtc-rust/sdk
echo "==> link sdk for signaling build (devDependency; workspace only)"
npm install --ignore-scripts --no-audit --no-fund \
  --workspace=@node-webrtc-rust/sdk \
  --workspace=@node-webrtc-rust/signaling

echo "==> build TypeScript (sdk first — signaling imports sdk types)"
npm run build --workspace=@node-webrtc-rust/sdk
npm run build --workspace=@node-webrtc-rust/signaling

echo "==> napi prepublish"
cd "$BINDINGS"
npx napi prepublish -t npm

FLAGS=(--access public)
if [[ -n "$NPM_OTP" ]]; then
  FLAGS+=(--otp="$NPM_OTP")
  export NPM_CONFIG_OTP="$NPM_OTP"
  echo "==> Using npm 2FA one-time password from --otp / NPM_OTP"
elif [[ "$DRY_RUN" != true ]]; then
  echo "==> npm 2FA: if publish fails with EOTP, re-run with --otp=<6-digit code> from your authenticator"
fi
if [[ "$DRY_RUN" == true ]]; then
  FLAGS+=(--dry-run)
  echo "==> npm publish (dry-run)"
else
  echo "==> npm publish (first-time scoped packages need --access public)"
  echo "    Ensure the @node-webrtc-rust scope exists on npmjs.com for your account"
fi

publish_dir() {
  local dir="$1"
  local label="$2"
  local extra_flags=("${@:3}")
  if [[ ! -f "${dir}package.json" ]]; then
    return 0
  fi
  shopt -s nullglob
  local nodes=("$dir"*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "Missing .node in $dir (run napi artifacts / prepublish)" >&2
    exit 1
  fi
  echo "  $label"
  (cd "$dir" && npm publish "${FLAGS[@]}" "${extra_flags[@]}")
}

verify_on_registry() {
  local pkg="$1"
  [[ "$DRY_RUN" == true ]] && return 0
  bash "$ROOT/scripts/ci/wait-for-npm-package.sh" "$pkg" "$VERSION"
}

echo "==> Publish platform binding packages (must go first)"
for dir in "$BINDINGS"/npm/*/; do
  pkg=$(cd "$dir" && npm pkg get name | tr -d '"')
  publish_dir "$dir" "$pkg"
  verify_on_registry "$pkg"
done

echo "==> Publish @node-webrtc-rust/bindings"
echo "    (--ignore-scripts: prepublish already ran; avoids npm fetching optionalDeps from registry)"
publish_dir "$BINDINGS/" "@node-webrtc-rust/bindings" --ignore-scripts
verify_on_registry "@node-webrtc-rust/bindings"

echo "==> Publish @node-webrtc-rust/signaling (before sdk; no sdk in dependencies — peer only)"
publish_dir "$ROOT/packages/signaling" "@node-webrtc-rust/signaling" --ignore-scripts --omit=dev
verify_on_registry "@node-webrtc-rust/signaling"

echo "==> Publish @node-webrtc-rust/sdk (requires bindings + signaling on npm)"
publish_dir "$ROOT/packages/sdk" "@node-webrtc-rust/sdk" --ignore-scripts
verify_on_registry "@node-webrtc-rust/sdk"

echo "==> Done: $VERSION"
if [[ "$DRY_RUN" != true ]]; then
  echo "Optional: git tag release/$VERSION && git push origin release/$VERSION"
fi
