/* eslint-disable no-console */

const { existsSync, readFileSync } = require('fs')
const { join } = require('path')

const { platform, arch } = process

let nativeBinding = null
let loadError = null

const platformPackages = {
  'darwin-arm64': '@node-webrtc-rust/bindings-darwin-arm64',
  'darwin-x64': '@node-webrtc-rust/bindings-darwin-x64',
  'linux-x64-glibc': '@node-webrtc-rust/bindings-linux-x64-gnu',
  'linux-x64-musl': '@node-webrtc-rust/bindings-linux-x64-musl',
  'linux-arm64-glibc': '@node-webrtc-rust/bindings-linux-arm64-gnu',
  'win32-x64': '@node-webrtc-rust/bindings-win32-x64-msvc',
}

function isMusl() {
  if (platform !== 'linux') return false
  try {
    return readFileSync('/usr/bin/ldd', 'utf8').includes('musl')
  } catch {
    const { isMusl: isMuslFromChild } = (() => {
      try {
        return require('detect-libc')
      } catch {
        return { isMusl: false }
      }
    })()
    return isMuslFromChild
  }
}

function getPackageKey() {
  if (platform === 'linux') {
    const libc = isMusl() ? 'musl' : 'glibc'
    return `${platform}-${arch}-${libc}`
  }
  return `${platform}-${arch}`
}

function loadNativeBinding() {
  const key = getPackageKey()
  const packageName = platformPackages[key]

  if (packageName) {
    try {
      nativeBinding = require(packageName)
      return
    } catch (e) {
      loadError = e
    }
  }

  // Fallback: try loading a local .node file (development builds)
  const localFile = join(__dirname, `node-webrtc-rust.${platform}-${arch}.node`)
  if (existsSync(localFile)) {
    try {
      nativeBinding = require(localFile)
      return
    } catch (e) {
      loadError = e
    }
  }

  // Fallback: generic local file
  const genericFile = join(__dirname, 'node-webrtc-rust.node')
  if (existsSync(genericFile)) {
    try {
      nativeBinding = require(genericFile)
      return
    } catch (e) {
      loadError = e
    }
  }
}

loadNativeBinding()

if (!nativeBinding) {
  const key = getPackageKey()
  const packageName = platformPackages[key]
  const hint = packageName
    ? `Expected optional package "${packageName}" to be installed.`
    : `No prebuilt binary available for ${platform}-${arch}.`

  throw new Error(
    `Failed to load native binding for node-webrtc-rust.\n` +
    `${hint}\n` +
    `Run \`npm run build\` in packages/bindings if developing locally.\n` +
    (loadError ? `Original error: ${loadError.message}` : '')
  )
}

module.exports = nativeBinding
