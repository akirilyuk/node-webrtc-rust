#!/usr/bin/env bash
# Local release: publish all @node-webrtc-rust/* packages from this machine.
#
# Usage:
#   ./scripts/release-local.sh <version> <npm-token> [--dry-run] [--otp=CODE]
#
# The script will:
#   1. Detect host OS + arch
#   2. Use existing .node from packages/bindings/ if one matches, else rebuild
#   3. Bump versions, build TS, publish in correct dependency order
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINDINGS="$ROOT/packages/bindings"

VERSION="${1:-}"
NPM_TOKEN="${2:-}"
shift 2 2>/dev/null || true

DRY_RUN=false
NPM_OTP=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true ;;
    --otp=*) NPM_OTP="${1#*=}" ;;
    --otp) shift; NPM_OTP="${1:-}" ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
  shift
done

if [[ -z "$VERSION" || -z "$NPM_TOKEN" ]]; then
  echo "Usage: $0 <version> <npm-token> [--dry-run] [--otp=CODE]" >&2
  echo "" >&2
  echo "Examples:" >&2
  echo "  $0 0.2.0 npm_xxxx" >&2
  echo "  $0 0.2.0 npm_xxxx --dry-run" >&2
  echo "  $0 0.2.0 npm_xxxx --otp=123456" >&2
  exit 1
fi

# ─── Dependency order check ───────────────────────────────────────────────────
# Verify publish order is safe (no hard circular production deps).
check_dep_order() {
  local sdk_deps sig_deps
  sdk_deps=$(node -e "const p=require('$ROOT/packages/sdk/package.json'); console.log(Object.keys(p.dependencies||{}).join(' '))")
  sig_deps=$(node -e "const p=require('$ROOT/packages/signaling/package.json'); console.log(Object.keys(p.dependencies||{}).join(' '))")

  if echo "$sig_deps" | grep -q "@node-webrtc-rust/sdk"; then
    echo "ERROR: @node-webrtc-rust/signaling has a PRODUCTION dependency on sdk." >&2
    echo "This creates a hard circular dep (sdk also depends on signaling)." >&2
    echo "Move sdk to peerDependencies in signaling before publishing." >&2
    exit 1
  fi
  echo "  ok — no hard circular production dependencies"
}

# ─── Detect host platform ────────────────────────────────────────────────────
detect_node_filename() {
  local os arch suffix
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)

  case "$os" in
    darwin)
      case "$arch" in
        arm64|aarch64) suffix="darwin-arm64" ;;
        x86_64)        suffix="darwin-x64" ;;
        *) echo "Unsupported macOS arch: $arch" >&2; exit 1 ;;
      esac
      ;;
    linux)
      local libc="gnu"
      if ldd --version 2>&1 | grep -qi musl || ([ -f /etc/os-release ] && grep -qi alpine /etc/os-release); then
        libc="musl"
      fi
      case "$arch" in
        x86_64)        suffix="linux-x64-${libc}" ;;
        aarch64|arm64) suffix="linux-arm64-${libc}" ;;
        *) echo "Unsupported Linux arch: $arch" >&2; exit 1 ;;
      esac
      ;;
    mingw*|msys*|cygwin*)
      suffix="win32-x64-msvc"
      ;;
    *) echo "Unsupported OS: $os" >&2; exit 1 ;;
  esac

  echo "node-webrtc-rust.${suffix}.node"
}

# ─── Ensure native binary exists ─────────────────────────────────────────────
ensure_native_binary() {
  local node_file="$1"
  local found=""

  # Check packages/bindings/ root
  if [[ -f "$BINDINGS/$node_file" ]]; then
    found="$BINDINGS/$node_file"
  fi

  # Check packages/bindings/prebuilt/
  if [[ -z "$found" ]] && [[ -f "$BINDINGS/prebuilt/$node_file" ]]; then
    found="$BINDINGS/prebuilt/$node_file"
    cp "$found" "$BINDINGS/$node_file"
  fi

  # Check artifacts dir
  if [[ -z "$found" ]]; then
    shopt -s nullglob
    for f in "$BINDINGS"/artifacts/*/"$node_file"; do
      found="$f"
      cp "$found" "$BINDINGS/$node_file"
      break
    done
  fi

  if [[ -n "$found" ]]; then
    echo "  found: $node_file ($(dirname "$found" | xargs basename))"
    return 0
  fi

  return 1
}

build_native_binary() {
  echo "  building native binary (release, host-only)..."
  export CMAKE_POLICY_VERSION_MINIMUM=3.5
  export OPUS_STATIC=1
  (cd "$BINDINGS" && npm run build:local)
  echo "  build complete"
}

# ─── npm auth ────────────────────────────────────────────────────────────────
setup_npm_auth() {
  export NPM_TOKEN
  echo "//registry.npmjs.org/:_authToken=\${NPM_TOKEN}" > "$ROOT/.npmrc_release"
  export NPM_CONFIG_USERCONFIG="$ROOT/.npmrc_release"

  if ! npm whoami &>/dev/null; then
    echo "ERROR: npm auth failed. Check your token." >&2
    rm -f "$ROOT/.npmrc_release"
    exit 1
  fi
  echo "  authenticated as: $(npm whoami)"
}

cleanup_npm_auth() {
  rm -f "$ROOT/.npmrc_release"
}
trap cleanup_npm_auth EXIT

# ─── Version bump (direct JSON edits — avoids npm registry resolution) ────────
set_json_field() {
  local file="$1" field="$2" value="$3"
  node -e "
    const fs = require('fs');
    const p = JSON.parse(fs.readFileSync('$file','utf8'));
    const keys = '$field'.split('.');
    let obj = p;
    for (let i = 0; i < keys.length - 1; i++) {
      if (!obj[keys[i]]) obj[keys[i]] = {};
      obj = obj[keys[i]];
    }
    obj[keys[keys.length-1]] = '$value';
    fs.writeFileSync('$file', JSON.stringify(p, null, 2) + '\n');
  "
}

bump_versions() {
  echo "==> Bumping all packages to $VERSION"

  # Core packages
  set_json_field "$BINDINGS/package.json" "version" "$VERSION"
  set_json_field "$ROOT/packages/sdk/package.json" "version" "$VERSION"
  set_json_field "$ROOT/packages/signaling/package.json" "version" "$VERSION"

  # Cross-references in sdk
  set_json_field "$ROOT/packages/sdk/package.json" "dependencies.@node-webrtc-rust/bindings" "$VERSION"
  set_json_field "$ROOT/packages/sdk/package.json" "dependencies.@node-webrtc-rust/signaling" "$VERSION"

  # Optional deps in bindings
  for opt in \
    bindings-darwin-arm64 \
    bindings-darwin-x64 \
    bindings-linux-x64-gnu \
    bindings-linux-x64-musl \
    bindings-linux-arm64-gnu \
    bindings-win32-x64-msvc; do
    set_json_field "$BINDINGS/package.json" "optionalDependencies.@node-webrtc-rust/${opt}" "$VERSION"
  done

  # Platform package versions
  for dir in "$BINDINGS"/npm/*/; do
    if [[ -f "${dir}package.json" ]]; then
      set_json_field "${dir}package.json" "version" "$VERSION"
    fi
  done

  # Examples (private — no publish, just keep versions in sync)
  for dir in "$ROOT"/examples/*/; do
    if [[ -f "${dir}package.json" ]]; then
      set_json_field "${dir}package.json" "version" "$VERSION"
      # Update any @node-webrtc-rust/* deps in examples
      node -e "
        const fs = require('fs');
        const f = '${dir}package.json';
        const p = JSON.parse(fs.readFileSync(f,'utf8'));
        let changed = false;
        for (const depType of ['dependencies','devDependencies','peerDependencies']) {
          if (!p[depType]) continue;
          for (const k of Object.keys(p[depType])) {
            if (k.startsWith('@node-webrtc-rust/')) {
              p[depType][k] = '$VERSION';
              changed = true;
            }
          }
        }
        if (changed) fs.writeFileSync(f, JSON.stringify(p, null, 2) + '\n');
      "
    fi
  done
}

# ─── Build TypeScript ─────────────────────────────────────────────────────────
build_typescript() {
  echo "==> Building TypeScript packages"
  cd "$ROOT"
  # Use workspace symlinks already in place — no npm install (avoids registry lookups)
  npm run build --workspace=@node-webrtc-rust/sdk
  npm run build --workspace=@node-webrtc-rust/signaling
}

# ─── Publish ──────────────────────────────────────────────────────────────────
publish_pkg() {
  publish_pkg_with_extra "$@"
}

publish_pkg_with_extra() {
  local dir="$1"
  local label="$2"
  shift 2
  local flags=(--access public)

  if [[ "$DRY_RUN" == true ]]; then
    flags+=(--dry-run)
  fi
  if [[ -n "$NPM_OTP" ]]; then
    flags+=(--otp="$NPM_OTP")
  fi
  # Append any extra flags passed by caller
  if [[ $# -gt 0 ]]; then
    flags+=("$@")
  fi

  echo "  publishing $label@$VERSION"
  (cd "$dir" && npm publish "${flags[@]}")
}

verify_published() {
  local pkg="$1"
  [[ "$DRY_RUN" == true ]] && return 0
  bash "$ROOT/scripts/ci/wait-for-npm-package.sh" "$pkg" "$VERSION"
}

# ═══════════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════════

echo "╔══════════════════════════════════════════════════════════╗"
echo "║  Local Release: @node-webrtc-rust/* → $VERSION"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

echo "==> Checking dependency order"
check_dep_order

echo "==> Setting up npm auth"
setup_npm_auth

NODE_FILE=$(detect_node_filename)
echo "==> Host binary: $NODE_FILE"

if ! ensure_native_binary "$NODE_FILE"; then
  echo "  no prebuilt binary found — rebuilding"
  build_native_binary
fi

# Verify binary loads
echo "==> Verifying native binary loads"
node -e "require('$BINDINGS/index.js'); console.log('  ok — native binding loaded')"

bump_versions
build_typescript

# Distribute .node files into npm/ subdirs (replaces `napi prepublish` which also publishes)
echo "==> Distributing .node files to npm/ platform dirs"
for dir in "$BINDINGS"/npm/*/; do
  if [[ -f "${dir}package.json" ]]; then
    expected=$(node -e "console.log(require('${dir}package.json').main)")
    # Look in bindings root, then prebuilt/, then artifacts/
    src=""
    if [[ -f "$BINDINGS/$expected" ]]; then
      src="$BINDINGS/$expected"
    elif [[ -f "$BINDINGS/prebuilt/$expected" ]]; then
      src="$BINDINGS/prebuilt/$expected"
    else
      shopt -s nullglob
      for f in "$BINDINGS"/artifacts/*/"$expected"; do
        src="$f"; break
      done
    fi
    if [[ -n "$src" ]]; then
      cp -f "$src" "$dir"
      echo "  $expected → $(basename "$dir")/"
    else
      echo "  SKIP $(basename "$dir")/ — $expected not found"
    fi
  fi
done

echo "==> Publishing packages (order: platform bindings → bindings → signaling → sdk)"

if [[ "$DRY_RUN" == true ]]; then
  echo "  [DRY RUN MODE]"
fi

# 1. Platform-specific binding packages (only the ones that have a .node file)
for dir in "$BINDINGS"/npm/*/; do
  if [[ -f "${dir}package.json" ]]; then
    shopt -s nullglob
    nodes=("${dir}"*.node)
    if [[ ${#nodes[@]} -gt 0 ]]; then
      pkg=$(node -e "console.log(require('${dir}package.json').name)")
      publish_pkg "$dir" "$pkg"
      verify_published "$pkg"
    fi
  fi
done

# 2. Main bindings package (--ignore-scripts: skip prepublishOnly which re-publishes platform pkgs)
publish_pkg_with_extra "$BINDINGS" "@node-webrtc-rust/bindings" --ignore-scripts
verify_published "@node-webrtc-rust/bindings"

# 3. Signaling (before sdk — sdk depends on it)
publish_pkg_with_extra "$ROOT/packages/signaling" "@node-webrtc-rust/signaling" --ignore-scripts
verify_published "@node-webrtc-rust/signaling"

# 4. SDK (depends on bindings + signaling)
publish_pkg_with_extra "$ROOT/packages/sdk" "@node-webrtc-rust/sdk" --ignore-scripts
verify_published "@node-webrtc-rust/sdk"

echo ""
echo "═══════════════════════════════════════════════════════════"
if [[ "$DRY_RUN" == true ]]; then
  echo "  DRY RUN complete — nothing was published"
else
  echo "  Published @node-webrtc-rust/*@$VERSION"
  echo ""
  echo "  Next steps:"
  echo "    git add -A && git commit -m \"chore(repo): release $VERSION\""
  echo "    git tag release/$VERSION && git push origin release/$VERSION"
fi
echo "═══════════════════════════════════════════════════════════"
