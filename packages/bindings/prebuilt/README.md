# Optional cache for release script

**Preferred:** keep platform `.node` files directly in `packages/bindings/`:

```
packages/bindings/node-webrtc-rust.linux-x64-gnu.node
packages/bindings/node-webrtc-rust.linux-x64-musl.node
packages/bindings/node-webrtc-rust.linux-arm64-gnu.node
packages/bindings/node-webrtc-rust.darwin-x64.node
packages/bindings/node-webrtc-rust.darwin-arm64.node
packages/bindings/node-webrtc-rust.win32-x64-msvc.node
```

`scripts/release-publish.sh` picks these up automatically and skips compiling.

You can also use `prebuilt/bindings-<target>/` or `artifacts/bindings-<target>/` from a previous run.
