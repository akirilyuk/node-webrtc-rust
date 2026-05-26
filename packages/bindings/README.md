# @node-webrtc-rust/bindings

Native NAPI-RS bindings for node-webrtc-rust. This package provides the compiled `.node` addon that bridges Rust and Node.js — including W3C WebRTC APIs and conference room mixing.

## For end users

Just install the package — prebuilt binaries are resolved automatically via `optionalDependencies`:

```bash
npm install @node-webrtc-rust/bindings
```

No Rust toolchain required. The correct platform-specific binary is downloaded during install.

### Supported platforms

| OS      | Arch              | Package                                      |
| ------- | ----------------- | -------------------------------------------- |
| macOS   | arm64 (M1+)       | `@node-webrtc-rust/bindings-darwin-arm64`    |
| macOS   | x64 (Intel)       | `@node-webrtc-rust/bindings-darwin-x64`      |
| Linux   | x64 (glibc)       | `@node-webrtc-rust/bindings-linux-x64-gnu`   |
| Linux   | x64 (musl/Alpine) | `@node-webrtc-rust/bindings-linux-x64-musl`  |
| Linux   | arm64 (glibc)     | `@node-webrtc-rust/bindings-linux-arm64-gnu` |
| Windows | x64 (MSVC)        | `@node-webrtc-rust/bindings-win32-x64-msvc`  |

## For developers (building from source)

Prerequisites:

- Rust toolchain (stable, via [rustup](https://rustup.rs))
- Node.js >= 18
- `@napi-rs/cli` (installed as devDependency)

```bash
cd packages/bindings
npm install
npm run build:local        # release build for current platform (default)
npm run build:debug:local  # debug build — fastest iteration loop
```

For all platform targets (CI/publish only):

```bash
npm run build:all
```

This produces a `node-webrtc-rust.<platform>.node` file in the current directory, which the loader (`index.js`) picks up as a fallback when no platform package is installed.

### Debug logging

Set `WEBRTC_DEBUG=1` (or pass `debug: true` in `JsRTCConfiguration`) to emit `[webrtc-debug]` lines from native bindings and the Rust core. See the root README for details.

## How it works

The `index.js` loader resolves the native binding in this order:

1. Try requiring the platform-specific npm package (e.g., `@node-webrtc-rust/bindings-darwin-arm64`)
2. Try loading a local `.node` file matching the current platform (dev builds)
3. Try loading `node-webrtc-rust.node` (generic local build)

If none succeed, an error is thrown with instructions.

### TypeScript note

This package is the **only** Node-facing package that ships hand-written JavaScript (`index.js`) for the napi-rs prebuild loader. The loader re-exports the full native module (WebRTC peer APIs and conference room APIs). TypeScript consumers use the generated `index.d.ts`. Higher-level APIs live in `@node-webrtc-rust/sdk` (including `@node-webrtc-rust/sdk/conference`) and `@node-webrtc-rust/signaling`.

## Cross-compilation

CI builds all platform targets using GitHub Actions. Linux builds and tests pull `ghcr.io/akirilyuk/node-webrtc-rust/ci-build:latest` (rebuild by pushing to the `ci` branch — see [`docker/ci/Dockerfile`](../../docker/ci/Dockerfile) and [`.github/workflows/ci-image.yml`](../../.github/workflows/ci-image.yml)). macOS and Windows jobs use native runners. See [`.github/workflows/build.yml`](../../.github/workflows/build.yml).
