#!/usr/bin/env bash
# Tests for check-native-exports.mjs using a fake index.d.ts and fake loaders.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CHECK="$ROOT/scripts/ci/check-native-exports.mjs"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat >"$tmp/index.d.ts" <<'DTS'
export declare function foo(x: number): number
export interface Ignored { a: string }
export declare const enum Mode { A = 0 }
export declare class Bar {
  constructor()
  get size(): number
  baz(): void
  async qux(): Promise<void>
  static make(): Bar
}
DTS

good="$tmp/good.cjs"
cat >"$good" <<'JS'
class Bar { get size() { return 0 } baz() {} async qux() {} static make() { return new Bar() } }
module.exports = { foo() {}, Bar }
JS

noqux="$tmp/noqux.cjs"
cat >"$noqux" <<'JS'
class Bar { get size() { return 0 } baz() {} static make() { return new Bar() } }
module.exports = { foo() {}, Bar }
JS

nofoo="$tmp/nofoo.cjs"
cat >"$nofoo" <<'JS'
class Bar { get size() { return 0 } baz() {} async qux() {} static make() { return new Bar() } }
module.exports = { Bar }
JS

out="$(node "$CHECK" "$tmp/index.d.ts" "$good")"
grep -q 'OK: native exports match index.d.ts (1 functions, 1 classes)' <<<"$out"

set +e
err="$(node "$CHECK" "$tmp/index.d.ts" "$noqux" 2>&1 >/dev/null)"; ec=$?
set -e
[[ "$ec" == "1" ]] || { echo "expected exit 1 for missing Bar.qux, got $ec" >&2; exit 1; }
grep -q 'missing Bar.qux' <<<"$err"
grep -q 'run npm run build:native' <<<"$err"

set +e
err="$(node "$CHECK" "$tmp/index.d.ts" "$nofoo" 2>&1 >/dev/null)"; ec=$?
set -e
[[ "$ec" == "1" ]] || { echo "expected exit 1 for missing foo, got $ec" >&2; exit 1; }
grep -q 'missing foo' <<<"$err"

echo "OK: check-native-exports test"
