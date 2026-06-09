# Future plan: offline Sherpa STT models

**Status:** Draft — not implemented  
**Date:** 2026-06-09  
**Related:** [`crates/vendor-sherpa-onnx/README.md`](../crates/vendor-sherpa-onnx/README.md), [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../examples/shared/VOICE_VENDOR_REFERENCE.md), [`examples/shared/sherpa-local-model-catalog.json`](../examples/shared/sherpa-local-model-catalog.json)

---

## Summary

Today, `local-sherpa` STT supports **streaming Zipformer transducer** models only (`OnlineRecognizer` + encoder/decoder/joiner). Sherpa also ships **offline** ASR families (notably **Cohere Transcribe**) that use a different ONNX layout and `OfflineRecognizer`. They are **not** drop-in replacements for Kroko Zipformer paths.

This document describes why, what would need to change in `node-webrtc-rust`, and when offline Sherpa is worth building vs staying on streaming Zipformer or cloud STT.

---

## What works today (streaming transducer)

### Architecture

| Layer | Implementation |
| ----- | -------------- |
| Sherpa API | `OnlineRecognizer` + per-session `OnlineStream` |
| Model layout | `encoder.onnx`, `decoder.onnx`, `joiner.onnx`, `tokens.txt` |
| Loader | [`crates/vendor-sherpa-onnx/src/loader.rs`](../crates/vendor-sherpa-onnx/src/loader.rs) — hard-coded transducer config |
| Path resolver | [`crates/vendor-sherpa-onnx/src/model_paths.rs`](../crates/vendor-sherpa-onnx/src/model_paths.rs) — requires `joiner` |
| STT adapter | [`crates/vendor-sherpa-onnx/src/stt.rs`](../crates/vendor-sherpa-onnx/src/stt.rs) — `accept_waveform` + decode loop + partials |
| Pool | [`crates/vendor-sherpa-onnx/src/pool.rs`](../crates/vendor-sherpa-onnx/src/pool.rs) — shared `OnlineRecognizer` per `model_path` |
| VoiceAgent contract | Continuous PCM → `user_stt_partial` while speaking → `user_speech_final` on utterance end |

### Recommended English model

**Default:** `sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06` (Kroko Zipformer, ~119 MB compressed).

This is the **best supported English streaming model** in our catalog. The older `…-en-2023-06-26` baseline is larger and less accurate — do not treat it as an upgrade path.

See [`examples/shared/sherpa-local-model-catalog.json`](../examples/shared/sherpa-local-model-catalog.json) for per-language streaming bundles.

---

## What “offline Sherpa” means (non-Zipformer)

Sherpa publishes ASR models that are **not** streaming transducers. The primary candidate called out in our catalog today is **Cohere Transcribe**.

### Cohere Transcribe (example bundle)

| Property | Streaming Zipformer (today) | Cohere Transcribe (offline) |
| -------- | --------------------------- | --------------------------- |
| Sherpa class | `OnlineRecognizer` | `OfflineRecognizer` |
| Streaming | Native token-by-token partials | **Offline / non-streaming** |
| ONNX files | encoder + decoder + **joiner** | encoder + decoder (no joiner) |
| Example bundle | `…-en-kroko-2025-08-06` | `sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01` |
| Languages | Per-language Kroko bundles | **One model, 14 languages** (en, de, es, fr, it, pt, pl, nl, el, ar, zh, ja, ko, vi) |
| Extra options | — | `language`, punctuation, inverse text normalization (ITN) |
| Sherpa docs | [Streaming Zipformer](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/index.html) | [Cohere Transcribe](https://k2-fsa.github.io/sherpa/onnx/cohere_transcribe/index.html) |

**Important:** Sherpa describes Cohere Transcribe as an **offline non-streaming** model. Decoding requires specifying the spoken language (e.g. `en`). It is wired in Sherpa via `OfflineRecognizerConfig.model_config.cohere_transcribe`, not `model_config.transducer`.

### Why it cannot work with current `local-sherpa` STT

1. **Path resolution fails** — we look for `joiner.onnx`; Cohere bundles use `encoder.int8.onnx` + `decoder.int8.onnx` only.
2. **Wrong recognizer type** — `create_online_recognizer()` always builds `OnlineRecognizer` with transducer fields.
3. **Wrong runtime semantics** — `SherpaStt` calls `OnlineStream::accept_waveform` and polls partials; offline models decode buffered utterances via `OfflineStream`.

Pointing `SHERPA_STT_MODEL_PATH` at a Cohere directory today will fail at startup or produce nonsense — **not** a config-only change.

### Other offline / non-transducer families (out of scope for v1)

Sherpa also ships Whisper, Paraformer, SenseVoice, NeMo, etc. Each has its own `OfflineRecognizer` config subtree. This plan focuses on **Cohere Transcribe first** because:

- It is documented and released on [asr-models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models).
- It covers languages **missing** from our streaming Zipformer catalog (Portuguese, Italian, Greek, Korean, …).
- Our catalog already notes it as “not wired to local-sherpa yet” ([`sherpa-local-model-catalog.json`](../examples/shared/sherpa-local-model-catalog.json)).

Whisper/Moonshine remain separate follow-ups (see [`development/node-webrtc-rust/followups/2026-05-28-local-whisper-stt.md`](../../development/node-webrtc-rust/followups/2026-05-28-local-whisper-stt.md) in the development repo).

---

## Is offline Sherpa “better accuracy” for English voice agents?

**Not automatically.** For **English-only, real-time** voice agents:

| Option | Trade-off |
| ------ | --------- |
| **Kroko streaming Zipformer** (current) | Best fit for live mic, partials, barge-in; already the top English local model we support |
| **Cohere offline** | May improve full-utterance accuracy on some clips; **no true streaming partials**; higher end-of-phrase latency |
| **Cloud STT** (Deepgram, OpenAI, …) | Often highest accuracy; API keys, network latency, audio leaves your worker |

Cohere’s main win in our stack is **language coverage** and **batch/offline transcription**, not a free upgrade over Kroko for live English conversation.

---

## Product impact on VoiceAgent

`VoiceAgent` + browser clients assume:

- **`user_stt_partial`** events while the user speaks (UI feedback, semantic barge-in).
- Low-latency streaming finals aligned with VAD gate hold / `finalize_utterance`.

Offline Sherpa fits **utterance-at-a-time** STT:

| Behavior | Streaming Zipformer | Offline (Cohere) |
| -------- | ------------------- | ---------------- |
| Partials during speech | Yes | No (unless we fake with chunked offline decode) |
| Final transcript timing | Continuous + endpoint | After VAD utterance boundary (or chunk boundary) |
| Barge-in (`requireSttPartial`) | Works with live partials | Needs redesign or relaxed barge-in rules |
| Latency profile | Low, steady | Spikes at utterance end (decode whole buffer) |
| CPU | Steady decode per frame | Bursts per utterance |

### Integration strategies

**Strategy A — Utterance decode (recommended MVP)**

- Buffer mono PCM for the open STT gate (reuse existing VAD + pre-roll in `crates/speech`).
- On `finalize_utterance`, run one `OfflineRecognizer` decode on the buffer.
- Emit **`user_speech_final` only** (no partials, or optional “fake partial” = last final text).
- Document that `events.mode` / barge-in presets differ for offline STT.

**Strategy B — Chunked offline “pseudo-streaming”**

- Run offline decode on sliding windows (e.g. every 500 ms of speech).
- Higher CPU; partials are approximate; more complex dedup vs finals.
- Defer unless a product requirement demands partials with offline models.

**Strategy C — Separate provider / example**

- New provider id e.g. `local-sherpa-offline` or `local-sherpa-cohere`.
- Keeps `local-sherpa` semantics unchanged for streaming Zipformer.
- Clearer for SDK users and runner config.

---

## Proposed implementation plan

### Phase 0 — Design decisions

- [ ] Choose provider surface: extend `local-sherpa` with `stt.modelKind` vs new `SttVendor` enum value.
- [ ] Choose integration strategy (A utterance-only vs B chunked partials).
- [ ] Define barge-in / partial behavior for offline STT (disable `requireSttPartial`? VAD-only barge-in?).
- [ ] Confirm minimum `sherpa-onnx` crate version with Rust `OfflineRecognizer` + Cohere config (currently pinned `1.13.2` in [`crates/vendor-sherpa-onnx/Cargo.toml`](../crates/vendor-sherpa-onnx/Cargo.toml)).

### Phase 1 — Model discovery & loader (`vendor-sherpa-onnx`)

- [ ] **`resolve_offline_cohere_paths()`** — `encoder*.onnx`, `decoder*.onnx`, `tokens.txt` (no joiner).
- [ ] **`create_offline_cohere_recognizer()`** — populate `OfflineRecognizerConfig.model_config.cohere_transcribe` (language, `use_punct`, `use_itn`).
- [ ] **Model kind detection** — e.g. `SHERPA_STT_MODEL_KIND=transducer|cohere-transcribe` or auto-detect from directory contents (prefer explicit env for production).
- [ ] Unit tests: loader builds recognizer when pointed at a downloaded Cohere bundle (CI: optional ignored test with model path env, same pattern as TTS smoke).

### Phase 2 — STT adapter (`SherpaOfflineStt`)

- [ ] Implement `SttProvider` for offline decode:
  - `start()` → create `OfflineRecognizer` + `OfflineStream` (from pool).
  - `push_audio()` → append to utterance buffer (respect `gate_stt` — only while gate open).
  - `poll_transcript()` → return queued final (and optional partials if Strategy B).
  - `finalize_utterance()` → `AcceptWaveform` on buffer, `Decode`, read result, emit `Final`.
- [ ] **`SherpaModelPool`** — separate cache for `OfflineRecognizer` keyed by `(model_path, language, punct, itn)`.
- [ ] Threading: keep `spawn_blocking` + decode semaphore (same as streaming STT).

### Phase 3 — Config & bindings

- [ ] Rust `SttConfig` fields: `model_kind`, `cohere_language`, `use_punct`, `use_itn` (names TBD).
- [ ] NAPI / TS SDK: expose on `VoiceAgentConfig.stt` with JSDoc.
- [ ] Factory routing in `SherpaFactory`: transducer → existing `SherpaStt`, cohere → `SherpaOfflineStt`.

### Phase 4 — Tooling & docs

- [ ] Extend [`sherpa-local-model-catalog.json`](../examples/shared/sherpa-local-model-catalog.json) with Cohere entry + `download-stt:cohere` script.
- [ ] Update [`VOICE_VENDOR_REFERENCE.md`](../examples/shared/VOICE_VENDOR_REFERENCE.md) — mark Portuguese / Italian as available via offline Cohere when implemented.
- [ ] Example env: `SHERPA_STT_MODEL_KIND=cohere-transcribe`, `SHERPA_STT_LANGUAGE=pt`.
- [ ] Runner / cloud worker docs (separate repos) — model mount paths, no joiner validation.

### Phase 5 — Tests & E2E

- [ ] Rust unit tests: utterance buffer → final text (mock or small wav fixture).
- [ ] New or adapted Sherpa roundtrip script with **offline expectations** (no partial assertions during speech; longer finalize tolerance).
- [ ] Document CI policy: Cohere bundle optional in CI (large download) vs required for offline STT job.

---

## Acceptance criteria

1. `VoiceAgent` with Cohere config transcribes a full VAD utterance with **no cloud keys**.
2. Startup logs print **resolved absolute model path**, model kind, and language.
3. Missing / wrong layout (transducer dir with cohere kind) fails with a **clear config error**.
4. Streaming Zipformer path **unchanged** — existing roundtrip E2E still pass without Cohere env.
5. SDK + catalog document which languages move from “cloud only” to “local offline”.

---

## Non-goals (initial release)

- Bundling offline model weights in npm or git.
- Unified auto-switch between streaming and offline mid-session.
- Whisper.cpp / Moonshine vendors (separate follow-ups).
- Replacing Kroko as default English STT for browser voice agents.

---

## Alternatives (no new Sherpa architecture)

| Need | Approach |
| ---- | -------- |
| Best English, stay local, live partials | Keep **Kroko 2025-08-06** |
| Best accuracy, cloud OK | [`voice-agent-browser`](../examples/voice-agent-browser/README.md) + Deepgram / OpenAI / AssemblyAI |
| Portuguese / Italian / Hindi local | Cohere offline (this plan) or cloud until shipped |

---

## References

- Sherpa Cohere Transcribe docs: https://k2-fsa.github.io/sherpa/onnx/cohere_transcribe/index.html
- Model release: https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models (`sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01.tar.bz2`)
- Original streaming Sherpa plan: `development/node-webrtc-rust/plans/2026-05-28-vendor-sherpa-onnx.md`
- VAD / gate / pre-roll (unchanged for streaming): [`packages/sdk/VOICE-VAD-AND-BARGE-IN.md`](../packages/sdk/VOICE-VAD-AND-BARGE-IN.md)
