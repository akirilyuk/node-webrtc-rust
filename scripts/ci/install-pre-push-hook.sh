#!/usr/bin/env bash
# Install a git pre-push hook that runs the same fast gates as CI quality (scoped).
# One-time per clone: bash scripts/ci/install-pre-push-hook.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOOK="$ROOT/.git/hooks/pre-push"

if [[ ! -d "$ROOT/.git" ]]; then
  echo "error: not a git repository ($ROOT)" >&2
  exit 1
fi

cat >"$HOOK" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
echo "==> pre-push: ci:pre-push (lint + TS workspace build + helpers vitest when scoped)"
npm run ci:pre-push
EOF

chmod +x "$HOOK"
echo "Installed $HOOK"
echo "Disable for one push: git push --no-verify"
