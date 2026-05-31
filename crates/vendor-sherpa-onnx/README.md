# node-webrtc-rust-vendor-sherpa-onnx

Local **streaming speech-to-text** via [sherpa-onnx](https://crates.io/crates/sherpa-onnx) transducer models (Zipformer, etc.).

Included in the **default** native bindings — no optional Cargo feature.

## Model directory layout

### Streaming STT (Zipformer transducer)

Point `SttConfig.model_path` or `SHERPA_STT_MODEL_PATH` at a folder containing:

```text
tokens.txt
*encoder*.onnx
*decoder*.onnx
*joiner*.onnx
```

Filenames are matched flexibly (e.g. `encoder-epoch-99-avg-1.int8.onnx`).

### Offline TTS (Piper/VITS)

Point `TtsConfig.model_path` or `SHERPA_TTS_MODEL_PATH` at a folder containing:

```text
tokens.txt
*.onnx              (single VITS/Piper model — not STT encoder/decoder/joiner)
espeak-ng-data/     (phoneme data — included in Sherpa tts-models bundles)
```

Optional: `SHERPA_TTS_SPEAKER` / `tts.voice` for multi-speaker Piper models (speaker id, default `0`).

Download voices via [`voice-agent-local-sherpa`](../../examples/voice-agent-local-sherpa/README.md) (`download-tts:*` scripts). Catalog: [`examples/shared/sherpa-tts-model-catalog.json`](../../examples/shared/sherpa-tts-model-catalog.json).

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

See [`examples/voice-agent-local-sherpa`](../../examples/voice-agent-local-sherpa/README.md) — per-language download scripts, `SHERPA_STT_MODEL_PATH`, `SHERPA_STT_LANGUAGE`, and WebRTC browser client.

**Why prefer `local-sherpa` for STT?** Free open-weight models, no cloud STT API keys, **better privacy** (user audio is not sent to third-party STT vendors), and **lower latency** (no cloud STT round-trip). See the example README for the recommended flow.

### Multilingual model downloads

Pinned streaming Zipformer bundles (Spanish, French, German, Chinese, Japanese, Arabic, Russian, Bengali, and more) are listed in [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../../examples/shared/VOICE_VENDOR_REFERENCE.md#local-sherpa-onnx--multilingual-models). Catalog source: [`examples/shared/sherpa-local-model-catalog.json`](../../examples/shared/sherpa-local-model-catalog.json).

```bash
npm run download-stt:list --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-stt:de --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
export SHERPA_STT_MODEL_PATH="…/.models/sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06"
export SHERPA_STT_LANGUAGE=de
```

Hindi, Portuguese, and Italian are not yet available as dedicated streaming Zipformer bundles in the official `asr-models` release set wired to this adapter — use cloud STT or see the vendor reference for alternatives.

| Product             | Docs                                                                                |
| ------------------- | ----------------------------------------------------------------------------------- |
| Sherpa-ONNX project | [GitHub](https://github.com/k2-fsa/sherpa-onnx)                                     |
| Streaming ASR guide | [k2-fsa.github.io/sherpa/onnx](https://k2-fsa.github.io/sherpa/onnx/)               |
| Pre-trained models  | [ASR model releases](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models) |
| Rust crate          | [docs.rs/sherpa-onnx](https://docs.rs/sherpa-onnx/)                                 |

See also [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md).

## Multi-session scaling (current vs planned)

**Pooled (default):** a process-wide **`SherpaModelPool`** shares one `OnlineRecognizer` per STT `model_path` and one `OfflineTts` per TTS model directory. Each `VoiceAgent` / `SherpaStt` session still owns its own `OnlineStream`; TTS uses a shared engine with a synthesis mutex and optional concurrency limits.

| Variable                            | Default           | Purpose                                             |
| ----------------------------------- | ----------------- | --------------------------------------------------- |
| `SHERPA_POOL_MAX_CONCURRENT_DECODE` | CPU count (min 1) | Cap parallel STT decode work                        |
| `SHERPA_POOL_MAX_CONCURRENT_TTS`    | `2`               | Cap parallel TTS generations                        |
| `SHERPA_STT_NUM_THREADS`            | ORT default       | `OnlineRecognizer` intra-op threads (`0` = default) |
| `SHERPA_TTS_NUM_THREADS`            | `2`               | `OfflineTts` intra-op threads                       |

Design notes and RAM/CPU tables: `development/node-webrtc-rust/plans/2026-05-31-sherpa-shared-model-pool.md`

Integration tests (require downloaded weights): `cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test pool_sharing -- --ignored`

## Threading

Sherpa C API calls run inside `tokio::task::spawn_blocking`. Do not invoke `OnlineRecognizer` directly from async tasks.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-sherpa-onnx
```

No model weights required for unit tests (factory + missing-path errors).
