#!/usr/bin/env bash
# Hermetic shell tests for compute-pr-job-gates.sh + version-only-diff.py.
# Every case builds its own temp git repo; no origin/main, repo history, or rg needed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GATES="$ROOT/scripts/ci/compute-pr-job-gates.sh"

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 required" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

GIT=(git -c user.name=t -c user.email=t@example.invalid)

assert_contains() {
  local haystack="$1" needle="$2"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "expected output to contain: $needle" >&2
    echo "$haystack" >&2
    exit 1
  fi
}

# assert_suites <output> <expected>: the output has exactly one test_suites= line with this value.
assert_suites() {
  local out="$1" expected="$2" lines
  lines="$(grep -E '^test_suites=' <<<"$out" || true)"
  if [[ "$lines" != "test_suites=$expected" ]]; then
    echo "expected exactly: test_suites=$expected" >&2
    echo "got: ${lines:-<none>}" >&2
    echo "$out" >&2
    exit 1
  fi
}

assert_not_contains() {
  local haystack="$1" needle="$2"
  if [[ "$haystack" == *"$needle"* ]]; then
    echo "expected output NOT to contain: $needle" >&2
    echo "$haystack" >&2
    exit 1
  fi
}

# make_repo <name>: empty repo with one base commit; prints the dir.
make_repo() {
  local dir="$tmp/$1"
  git init -q "$dir"
  "${GIT[@]}" -C "$dir" commit -q --allow-empty -m base
  echo "$dir"
}

# commit_files <dir> <message> <path>...: writes a small file for each path and commits.
commit_files() {
  local dir="$1" msg="$2"
  shift 2
  local p
  for p in "$@"; do
    mkdir -p "$dir/$(dirname "$p")"
    echo "x $msg" >"$dir/$p"
  done
  git -C "$dir" add -A
  "${GIT[@]}" -C "$dir" commit -q -m "$msg"
}

# gates <dir> <base> <head>
gates() {
  (cd "$1" && bash "$GATES" "$2" "$3")
}

# Case with a single commit on top of the base commit.
run_paths_case() {
  local name="$1"
  shift
  local dir base head
  dir="$(make_repo "$name")"
  base="$(git -C "$dir" rev-parse HEAD)"
  commit_files "$dir" "change" "$@"
  head="$(git -C "$dir" rev-parse HEAD)"
  gates "$dir" "$base" "$head"
}

# --- Fixtures for version-only cases -------------------------------------------------------
# write_fixture <dir> <own-version> <left-pad-version> <integrity-of-registry-binding>
write_fixture() {
  local dir="$1" v="$2" lp="$3" integ="$4"
  mkdir -p "$dir/packages/bindings"
  cat >"$dir/package.json" <<JSON
{"name":"node-webrtc-rust","version":"$v","workspaces":["packages/*"],"devDependencies":{"left-pad":"1.0.0"}}
JSON
  cat >"$dir/packages/bindings/package.json" <<JSON
{"name":"@node-webrtc-rust/bindings","version":"$v","optionalDependencies":{"@node-webrtc-rust/bindings-linux-x64-gnu":"$v"}}
JSON
  cat >"$dir/package-lock.json" <<JSON
{"name":"node-webrtc-rust","version":"$v","lockfileVersion":3,"requires":true,"packages":{"":{"name":"node-webrtc-rust","version":"$v","workspaces":["packages/*"],"devDependencies":{"left-pad":"1.0.0"}},"packages/bindings":{"name":"@node-webrtc-rust/bindings","version":"$v","optionalDependencies":{"@node-webrtc-rust/bindings-linux-x64-gnu":"$v"}},"node_modules/@node-webrtc-rust/bindings":{"resolved":"packages/bindings","link":true},"node_modules/@node-webrtc-rust/bindings-linux-x64-gnu":{"version":"$v","resolved":"https://registry.npmjs.org/@node-webrtc-rust/bindings-linux-x64-gnu/-/bindings-linux-x64-gnu-$v.tgz","integrity":"$integ","optional":true},"node_modules/left-pad":{"version":"$lp","resolved":"https://registry.npmjs.org/left-pad/-/left-pad-$lp.tgz","integrity":"sha512-bbb","dev":true}}}
JSON
}

# lock_repo <name>: repo whose base commit holds the 0.11.1 fixture; prints the dir.
lock_repo() {
  local dir="$tmp/$1"
  git init -q "$dir"
  write_fixture "$dir" 0.11.1 1.0.0 sha512-aaa
  git -C "$dir" add -A
  "${GIT[@]}" -C "$dir" commit -q -m base
  echo "$dir"
}

commit_all() {
  git -C "$1" add -A
  "${GIT[@]}" -C "$1" commit -q -m "$2"
}

# --- 1. docs-only incl. non-Markdown docs data ---------------------------------------------
out="$(run_paths_case docs docs/a.md README.md docs/perf-data/x.json docs/perf-data/y.txt)"
assert_contains "$out" "run_quality=true"
assert_contains "$out" "run_compile=false"
assert_contains "$out" "run_build_ts=false"
assert_contains "$out" "run_test=false"
assert_contains "$out" "skip_test=true"
assert_contains "$out" "version_only=false"
assert_suites "$out" ""
echo "ok: docs-only diff (incl. docs/perf-data) -> quality only"

# --- 2. crates change -----------------------------------------------------------------------
out="$(run_paths_case crates crates/speech/src/x.rs)"
assert_contains "$out" "run_compile=true"
assert_contains "$out" "run_test=true"
assert_contains "$out" "skip_test=false"
assert_contains "$out" "needs_native=true"
echo "ok: crates change -> compile + test"

# --- 3. TS-only PR compiles native ----------------------------------------------------------
out="$(run_paths_case ts packages/sdk/src/x.ts)"
assert_contains "$out" "run_compile=true"
assert_contains "$out" "run_test=true"
assert_contains "$out" "run_build_ts=true"
echo "ok: TS-only change -> compile + build-ts + test"

# --- 4. workflow change runs everything -----------------------------------------------------
out="$(run_paths_case wf .github/workflows/build.yml)"
assert_contains "$out" "run_compile=true"
assert_contains "$out" "run_test=true"
echo "ok: workflow change -> full pipeline"

# --- 5. CI script change runs everything ----------------------------------------------------
out="$(run_paths_case ci scripts/ci/foo.sh)"
assert_contains "$out" "run_compile=true"
assert_contains "$out" "run_test=true"
echo "ok: CI script change -> full pipeline"

# --- 6. speech change only in the latest commit ---------------------------------------------
dir="$(make_repo latest)"
base="$(git -C "$dir" rev-parse HEAD)"
commit_files "$dir" "docs" docs/a.md
commit_files "$dir" "speech" crates/speech/src/y.rs
head="$(git -C "$dir" rev-parse HEAD)"
out="$(gates "$dir" "$base" "$head")"
assert_contains "$out" "run_compile=true"
assert_contains "$out" "run_test=true"
echo "ok: speech change in latest commit -> compile + test"

# --- 7. version-only lock PR ----------------------------------------------------------------
dir="$(lock_repo vo)"
base="$(git -C "$dir" rev-parse HEAD)"
write_fixture "$dir" 0.11.2 1.0.0 sha512-ccc
commit_all "$dir" bump
head="$(git -C "$dir" rev-parse HEAD)"
out="$(gates "$dir" "$base" "$head")"
assert_contains "$out" "version_only=true"
assert_contains "$out" "run_compile=false"
assert_contains "$out" "run_test=false"
assert_contains "$out" "skip_test=true"
assert_contains "$out" "run_build_ts=true"
assert_contains "$out" "run_quality=true"
assert_suites "$out" ""
echo "ok: version-only lock PR -> quality + build-ts only"

# --- 8. third-party bump is not version-only ------------------------------------------------
dir="$(lock_repo third)"
base="$(git -C "$dir" rev-parse HEAD)"
write_fixture "$dir" 0.11.2 1.0.1 sha512-ccc
commit_all "$dir" bump
head="$(git -C "$dir" rev-parse HEAD)"
out="$(gates "$dir" "$base" "$head")"
assert_contains "$out" "version_only=false"
assert_contains "$out" "run_test=true"
assert_contains "$out" "run_compile=true"
echo "ok: third-party bump -> full pipeline"

# --- 9. new dependency is not version-only --------------------------------------------------
dir="$(lock_repo newdep)"
base="$(git -C "$dir" rev-parse HEAD)"
write_fixture "$dir" 0.11.2 1.0.0 sha512-ccc
cat >"$dir/package.json" <<'JSON'
{"name":"node-webrtc-rust","version":"0.11.2","workspaces":["packages/*"],"devDependencies":{"left-pad":"1.0.0","right-pad":"1.0.0"}}
JSON
commit_all "$dir" bump
head="$(git -C "$dir" rev-parse HEAD)"
out="$(gates "$dir" "$base" "$head")"
assert_contains "$out" "version_only=false"
assert_contains "$out" "run_test=true"
echo "ok: new dependency -> full pipeline"

# --- 10. lock bump plus a crates file -------------------------------------------------------
dir="$(lock_repo withcrate)"
base="$(git -C "$dir" rev-parse HEAD)"
write_fixture "$dir" 0.11.2 1.0.0 sha512-ccc
mkdir -p "$dir/crates/core/src"
echo "x" >"$dir/crates/core/src/z.rs"
commit_all "$dir" bump
head="$(git -C "$dir" rev-parse HEAD)"
out="$(gates "$dir" "$base" "$head")"
assert_contains "$out" "version_only=false"
assert_contains "$out" "run_compile=true"
echo "ok: lock bump plus crates file -> full pipeline"

# --- 11. invalid JSON -----------------------------------------------------------------------
dir="$(lock_repo badjson)"
base="$(git -C "$dir" rev-parse HEAD)"
write_fixture "$dir" 0.11.2 1.0.0 sha512-ccc
printf '{"name":' >"$dir/packages/bindings/package.json"
commit_all "$dir" bump
head="$(git -C "$dir" rev-parse HEAD)"
out="$(gates "$dir" "$base" "$head")"
assert_contains "$out" "version_only=false"
assert_contains "$out" "run_test=true"
assert_not_contains "$out" "version_only=true"
echo "ok: invalid JSON -> full pipeline"

# --- 12. path-based suite selection (scripts/ci/pr-test-suites.map) -------------------------
out="$(run_paths_case suites_sdk packages/sdk/src/x.ts)"
assert_suites "$out" "node,sherpa"
out="$(run_paths_case suites_example examples/audio-cosine/src/x.ts)"
assert_suites "$out" "node"
out="$(run_paths_case suites_vendor crates/vendor-openai/src/x.rs)"
assert_suites "$out" "rust,node"
out="$(run_paths_case suites_core crates/core/src/x.rs)"
assert_suites "$out" "rust,node,sherpa"
out="$(run_paths_case suites_union examples/audio-cosine/src/x.ts crates/vendor-openai/src/x.rs)"
assert_suites "$out" "rust,node"
out="$(run_paths_case suites_unmapped scripts/perf/x.mjs)"
assert_suites "$out" "rust,node,sherpa"
out="$(run_paths_case suites_workflow .github/workflows/build.yml)"
assert_suites "$out" "rust,node,sherpa"
# A docs file next to a mapped code path does not widen the selection.
out="$(run_paths_case suites_docs_mixed docs/a.md examples/audio-cosine/src/x.ts)"
assert_suites "$out" "node"
echo "ok: test_suites follows the path map (union, unmapped = all, docs ignored)"

# --- 13. malformed map fails closed ---------------------------------------------------------
dir="$(make_repo badmap)"
base="$(git -C "$dir" rev-parse HEAD)"
commit_files "$dir" "change" crates/core/src/x.rs
head="$(git -C "$dir" rev-parse HEAD)"
for bad in "crates/core/* rust,node extra" "crates/core/*" "crates/core/* rust,bogus"; do
  scripts_copy="$tmp/badmap-scripts"
  rm -rf "$scripts_copy"
  mkdir -p "$scripts_copy"
  cp "$GATES" "$ROOT/scripts/ci/version-only-diff.py" "$scripts_copy/"
  printf '# comment\n\n%s\n' "$bad" >"$scripts_copy/pr-test-suites.map"
  if (cd "$dir" && bash "$scripts_copy/compute-pr-job-gates.sh" "$base" "$head") >/dev/null 2>"$tmp/badmap.err"; then
    echo "expected malformed map line to fail: $bad" >&2
    exit 1
  fi
  grep -q 'compute-pr-job-gates:' "$tmp/badmap.err" || {
    echo "expected an error message for: $bad" >&2
    cat "$tmp/badmap.err" >&2
    exit 1
  }
done
echo "ok: malformed map line -> exit non-zero"

# --- 14. not a git repository fails closed --------------------------------------------------
nogit="$(mktemp -d)"
if out="$(cd "$nogit" && GIT_CEILING_DIRECTORIES="$nogit/.." bash "$GATES" 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 2>&1)"; then
  echo "expected the gates script to fail outside a git repository" >&2
  echo "$out" >&2
  exit 1
fi
assert_contains "$out" "not a git repository"
rm -rf "$nogit"
echo "ok: not a git repository -> exit non-zero"

# --- 15. unknown base commit fails closed ----------------------------------------------------
dir="$(make_repo unknownbase)"
commit_files "$dir" "change" crates/core/src/x.rs
head="$(git -C "$dir" rev-parse HEAD)"
if out="$(cd "$dir" && bash "$GATES" 0000000000000000000000000000000000000001 "$head" 2>&1)"; then
  echo "expected an unknown base commit to fail" >&2
  echo "$out" >&2
  exit 1
fi
assert_not_contains "$out" "run_test="
assert_contains "$out" "::error::compute-pr-job-gates: git diff"
echo "ok: unknown base commit -> exit non-zero, no gates printed"

echo "compute-pr-job-gates.test.sh: all checks passed"
