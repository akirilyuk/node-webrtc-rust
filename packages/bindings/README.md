# @node-webrtc-rust/bindings

Native NAPI-RS bindings for node-webrtc-rust. This package provides the compiled `.node` addon that bridges Rust and Node.js.

## For end users

Just install the package — prebuilt binaries are resolved automatically via `optionalDependencies`:

```bash
npm install @node-webrtc-rust/bindings
```

No Rust toolchain required. The correct platform-specific binary is downloaded during install.

### Supported platforms

| OS | Arch | Package |
|----|------|---------|
| macOS | arm64 (M1+) | `@node-webrtc-rust/bindings-darwin-arm64` |
| macOS | x64 (Intel) | `@node-webrtc-rust/bindings-darwin-x64` |
| Linux | x64 (glibc) | `@node-webrtc-rust/bindings-linux-x64-gnu` |
| Linux | x64 (musl/Alpine) | `@node-webrtc-rust/bindings-linux-x64-musl` |
| Linux | arm64 (glibc) | `@node-webrtc-rust/bindings-linux-arm64-gnu` |
| Windows | x64 (MSVC) | `@node-webrtc-rust/bindings-win32-x64-msvc` |

## For developers (building from source)

Prerequisites:
- Rust toolchain (stable, via [rustup](https://rustup.rs))
- Node.js >= 18
- `@napi-rs/cli` (installed as devDependency)

```bash
cd packages/bindings
npm install
npm run build        # release build for current platform
npm run build:debug  # debug build for development
```

This produces a `node-webrtc-rust.<platform>.node` file in the current directory, which the loader (`index.js`) picks up as a fallback when no platform package is installed.

## How it works

The `index.js` loader resolves the native binding in this order:

1. Try requiring the platform-specific npm package (e.g., `@node-webrtc-rust/bindings-darwin-arm64`)
2. Try loading a local `.node` file matching the current platform (dev builds)
3. Try loading `node-webrtc-rust.node` (generic local build)

If none succeed, an error is thrown with instructions.

## Cross-compilation

CI builds all platform targets using GitHub Actions. Linux targets use `cargo-zigbuild` to pin a minimum glibc version. See `.github/workflows/build.yml` for the full matrix.
