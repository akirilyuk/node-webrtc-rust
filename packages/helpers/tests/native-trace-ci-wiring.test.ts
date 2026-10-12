/**
 * Guard for error type `net.connect-stall-no-timeout`: a loopback peer reached ICE `connected` but
 * its `connectionState` stayed `connecting`, and CI kept no native DTLS/ICE log, so the same
 * stall recurred three times (2026-10-10 .. 2026-10-12) without evidence.
 *
 * The native trace must stay wired end to end, and must stay bounded and metadata-only:
 * - `scripts/ci/run-pr-integration.sh` sets WNR_LOG_FILE (per worker `{pid}`) and a RUST_LOG that
 *   covers dtls, webrtc_dtls, webrtc::dtls_transport and webrtc_ice at trace for the helpers
 *   integration step, and deletes the files when the step passes;
 * - `.github/actions/ci-run-integration-tests/action.yml` uploads them with `if: failure()`;
 * - the bindings only install the logger when WNR_LOG_FILE is set, and only for transport crates.
 * The env gating itself is executed in `native-trace-gating.integration.test.ts`.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

const REPO_ROOT = join(__dirname, '..', '..', '..')
const read = (rel: string): string => readFileSync(join(REPO_ROOT, rel), 'utf8')

describe('native trace CI wiring', () => {
  const script = read('scripts/ci/run-pr-integration.sh')
  const action = read('.github/actions/ci-run-integration-tests/action.yml')
  const trace = read('packages/bindings/src/native_trace.rs')
  const runtime = read('packages/bindings/src/runtime.rs')

  it('sets WNR_LOG_FILE with a per-process placeholder for the helpers integration step', () => {
    const step = script.slice(script.indexOf('==> helpers native integration tests'))
    expect(step).toMatch(/WNR_LOG_FILE="\$NATIVE_TRACE_DIR\/native-\{pid\}\.log"/)
    expect(step).toMatch(
      /"helpers integration"[\s\S]*test:integration --workspace=@node-webrtc-rust\/helpers/,
    )
    // The env must be applied to the helpers step, not to the whole script.
    expect(script.slice(0, script.indexOf('==> helpers native integration tests'))).not.toMatch(
      /WNR_LOG_FILE/,
    )
  })

  it('traces dtls, webrtc_dtls, webrtc::dtls_transport and ice at trace', () => {
    const rustLog = /NATIVE_TRACE_RUST_LOG="([^"]+)"/.exec(script)?.[1] ?? ''
    const levels = new Map(rustLog.split(',').map((kv) => kv.split('=') as [string, string]))
    for (const target of ['dtls', 'webrtc_dtls', 'webrtc::dtls_transport', 'webrtc_ice']) {
      expect(levels.get(target), `${target} in RUST_LOG`).toBe('trace')
    }
    // Never a global level: that would also enable debug for every dependency.
    expect([...levels.keys()].every((k) => k.length > 0 && !['debug', 'trace'].includes(k))).toBe(
      true,
    )
    expect(script).toMatch(/RUST_LOG="\$NATIVE_TRACE_RUST_LOG"/)
  })

  it('keeps the trace only when the helpers integration step fails', () => {
    expect(script).toMatch(/then\s+rm -rf "\$NATIVE_TRACE_DIR"\s+else/)
    expect(script).toMatch(/exit "\$helpers_rc"/)
  })

  it('uploads the trace as an artifact on failure only', () => {
    const idx = action.indexOf('Upload native trace')
    expect(idx).toBeGreaterThan(-1)
    const step = action.slice(idx)
    expect(step).toMatch(/if: failure\(\)/)
    expect(step).toMatch(/uses: actions\/upload-artifact@v4/)
    expect(step).toMatch(/path: \.test-logs\/native-trace/)
    expect(step).toMatch(/if-no-files-found: ignore/)
    expect(step).toMatch(/retention-days: 7/)
  })

  it('installs the native logger only behind WNR_LOG_FILE, from the module init', () => {
    expect(trace).toMatch(/std::env::var\("WNR_LOG_FILE"\)/)
    expect(trace).toMatch(/let Ok\(template\) = [^;]+else \{\s+return;/)
    expect(runtime).toMatch(/crate::native_trace::install_from_env\(\)/)
  })

  it('writes transport targets only and bounds the file', () => {
    const allowed = /ALLOWED_TARGET_PREFIXES: &\[&str\] = &\[([\s\S]*?)\];/.exec(trace)?.[1] ?? ''
    const prefixes = [...allowed.matchAll(/"([^"]+)"/g)].map((m) => m[1])
    expect(prefixes).toEqual(
      expect.arrayContaining(['dtls', 'webrtc_dtls', 'webrtc', 'webrtc_ice']),
    )
    // Our own crates (speech, mixer, vendors) can carry transcripts or text: never allowlisted.
    expect(prefixes.filter((p) => p.startsWith('node_webrtc_rust'))).toEqual([])
    expect(trace).toMatch(/AllowlistLogger/)
    expect(trace).toMatch(/WNR_LOG_SEGMENT_BYTES/)
    expect(trace).toMatch(/older\.push\("\.1"\)/)
  })
})
