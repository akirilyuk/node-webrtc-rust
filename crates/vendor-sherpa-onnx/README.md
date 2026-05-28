# node-webrtc-rust-vendor-sherpa-onnx

Local **streaming speech-to-text** via [sherpa-onnx](https://crates.io/crates/sherpa-onnx) transducer models (Zipformer, etc.).

Included in the **default** native bindings — no optional Cargo feature.

## Model directory layout

Point `SttConfig.model_path` or `SHERPA_MODEL_PATH` at a folder containing:

```text
tokens.txt
*encoder*.onnx
*decoder*.onnx
*joiner*.onnx
```

Filenames are matched flexibly (e.g. `encoder-epoch-99-avg-1.int8.onnx`).

## Usage

Register `SherpaFactory` and select `SttVendor::LocalSherpa` (`local-sherpa` in JSON / SDK).

```rust
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use node_webrtc_rust_speech::pipeline::VendorFactory;

let factory = SherpaFactory;
let stt = factory.create_stt(&SttConfig {
    provider: SttVendor::LocalSherpa,
    model_path: Some("/path/to/model-dir".into()),
    language: Some("en".into()),
    ..Default::default()
})?;
```

## Browser example

See [`examples/voice-agent-local-sherpa`](../../examples/voice-agent-local-sherpa/README.md) — download script, `SHERPA_MODEL_PATH`, and WebRTC browser client.

| Product | Docs |
|---------|------|
| Sherpa-ONNX project | [GitHub](https://github.com/k2-fsa/sherpa-onnx) |
| Streaming ASR guide | [k2-fsa.github.io/sherpa/onnx](https://k2-fsa.github.io/sherpa/onnx/) |
| Pre-trained models | [ASR model releases](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models) |
| Rust crate | [docs.rs/sherpa-onnx](https://docs.rs/sherpa-onnx/) |

See also [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md).

## Threading

Sherpa C API calls run inside `tokio::task::spawn_blocking`. Do not invoke `OnlineRecognizer` directly from async tasks.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-sherpa-onnx
```

No model weights required for unit tests (factory + missing-path errors).
