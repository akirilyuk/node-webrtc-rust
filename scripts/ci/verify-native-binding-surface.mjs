#!/usr/bin/env node
/**
 * Ensure a native .node implements every JsPeerConnection method declared in
 * packages/bindings/index.d.ts (committed NAPI contract).
 *
 * Usage:
 *   node scripts/ci/verify-native-binding-surface.mjs [--target TRIPLE]
 *
 * With --target, checks the napi-rs platform artifact for that triple (required
 * for Linux cross-builds on x64-gnu runners). Uses runtime dlopen when the
 * host can load the binary; otherwise scans the ELF for exported JS names.
 *
 * Without --target, loads via packages/bindings/index.js (local dev).
 */
import { execSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '../..')
const bindingsDir = join(root, 'packages/bindings')
const dtsPath = join(bindingsDir, 'index.d.ts')

/** Rust triple -> napi-rs platform .node basename (see release-publish.sh). */
const TARGET_TO_NODE = {
  'x86_64-unknown-linux-gnu': 'node-webrtc-rust.linux-x64-gnu.node',
  'x86_64-unknown-linux-musl': 'node-webrtc-rust.linux-x64-musl.node',
  'aarch64-unknown-linux-gnu': 'node-webrtc-rust.linux-arm64-gnu.node',
  'aarch64-unknown-linux-musl': 'node-webrtc-rust.linux-arm64-musl.node',
  'x86_64-apple-darwin': 'node-webrtc-rust.darwin-x64.node',
  'aarch64-apple-darwin': 'node-webrtc-rust.darwin-arm64.node',
  'x86_64-pc-windows-msvc': 'node-webrtc-rust.win32-x64-msvc.node',
}

function parseArgs(argv) {
  let target = null
  for (let i = 2; i < argv.length; i++) {
    if (argv[i] === '--target' && argv[i + 1]) {
      target = argv[++i]
      continue
    }
    console.error(`Unknown argument: ${argv[i]}`)
    process.exit(1)
  }
  return { target }
}

function isMusl() {
  if (!process.report || typeof process.report.getReport !== 'function') {
    try {
      const lddPath = execSync('which ldd', { encoding: 'utf8' }).trim()
      return readFileSync(lddPath, 'utf8').includes('musl')
    } catch {
      return false
    }
  }
  const { glibcVersionRuntime } = process.report.getReport().header
  return !glibcVersionRuntime
}

/** True when Node on this host can dlopen the built artifact for `target`. */
function canLoadAtRuntime(target) {
  const { platform, arch } = process
  switch (target) {
    case 'x86_64-unknown-linux-gnu':
      return platform === 'linux' && arch === 'x64' && !isMusl()
    case 'x86_64-unknown-linux-musl':
      return platform === 'linux' && arch === 'x64' && isMusl()
    case 'aarch64-unknown-linux-gnu':
      return platform === 'linux' && arch === 'arm64' && !isMusl()
    case 'aarch64-unknown-linux-musl':
      return platform === 'linux' && arch === 'arm64' && isMusl()
    case 'x86_64-apple-darwin':
      return platform === 'darwin' && arch === 'x64'
    case 'aarch64-apple-darwin':
      return platform === 'darwin' && arch === 'arm64'
    case 'x86_64-pc-windows-msvc':
      return platform === 'win32' && arch === 'x64'
    default:
      return false
  }
}

function parsePeerConnectionMethods() {
  const dts = readFileSync(dtsPath, 'utf8')
  const classBlock = dts.match(/export declare class JsPeerConnection \{([\s\S]*?)\n\}/)?.[1]
  if (!classBlock) {
    console.error('JsPeerConnection block not found in index.d.ts')
    process.exit(1)
  }

  const methods = [
    ...classBlock.matchAll(/^\s+(?!get |set |constructor)(\w+)\(/gm),
    ...classBlock.matchAll(/^\s+(setOn\w+)\(/gm),
  ].map((m) => m[1])

  const unique = [...new Set(methods)]
  if (unique.length === 0) {
    console.error('No JsPeerConnection methods parsed from index.d.ts')
    process.exit(1)
  }
  return unique
}

function resolveNodePath(target) {
  if (target) {
    const basename = TARGET_TO_NODE[target]
    if (!basename) {
      console.error(`Unsupported verify --target: ${target}`)
      process.exit(1)
    }
    const nodePath = join(bindingsDir, basename)
    if (!existsSync(nodePath)) {
      console.error(`Expected native artifact missing: ${basename}`)
      process.exit(1)
    }
    return nodePath
  }

  const nodes = readdirSync(bindingsDir).filter((name) => name.endsWith('.node'))
  if (nodes.length === 1) {
    return join(bindingsDir, nodes[0])
  }

  return null
}

function verifyRuntime(methods, nodePath) {
  const require = createRequire(join(bindingsDir, 'index.js'))
  const bindings = nodePath ? require(nodePath) : require(join(bindingsDir, 'index.js'))

  let pc
  try {
    pc = new bindings.JsPeerConnection({})
  } catch (error) {
    console.error('Failed to construct JsPeerConnection:', error)
    process.exit(1)
  }

  const missing = methods.filter((name) => typeof pc[name] !== 'function')
  if (missing.length > 0) {
    console.error('Native .node missing NAPI methods declared in index.d.ts:')
    for (const name of missing) {
      console.error(`  - ${name}`)
    }
    process.exit(1)
  }
}

function verifyStatic(methods, nodePath, target) {
  const bytes = readFileSync(nodePath)
  const missing = methods.filter((name) => !bytes.includes(name))
  if (missing.length > 0) {
    console.error(
      `Native .node (${TARGET_TO_NODE[target]}) missing symbols for index.d.ts methods:`,
    )
    for (const name of missing) {
      console.error(`  - ${name}`)
    }
    process.exit(1)
  }
}

const { target } = parseArgs(process.argv)
const methods = parsePeerConnectionMethods()
const nodePath = resolveNodePath(target)

if (target && canLoadAtRuntime(target)) {
  verifyRuntime(methods, nodePath)
  console.log(
    `Native binding OK (runtime) — ${methods.length} JsPeerConnection methods verified (${TARGET_TO_NODE[target]})`,
  )
} else if (target) {
  verifyStatic(methods, nodePath, target)
  console.log(
    `Native binding OK (static) — ${methods.length} JsPeerConnection methods verified (${TARGET_TO_NODE[target]})`,
  )
} else {
  verifyRuntime(methods, nodePath)
  console.log(`Native binding OK — ${methods.length} JsPeerConnection methods verified`)
}
