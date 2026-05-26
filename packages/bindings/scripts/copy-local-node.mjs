import { copyFileSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const src = join(root, 'node-webrtc-rust.node')

const tripleByPlatform = {
  'darwin-arm64': 'darwin-arm64',
  'darwin-x64': 'darwin-x64',
  'linux-arm64': 'linux-arm64-gnu',
  'linux-x64': 'linux-x64-gnu',
  'win32-x64': 'win32-x64-msvc',
}

const triple = tripleByPlatform[`${process.platform}-${process.arch}`]
if (!triple || !existsSync(src)) {
  process.exit(0)
}

copyFileSync(src, join(root, `node-webrtc-rust.${triple}.node`))
