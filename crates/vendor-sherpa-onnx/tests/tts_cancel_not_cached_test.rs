//! B2: a cancelled progressive synthesis must not be stored in the phrase cache.
//!
//! The cache lives in the speech crate (`CachingTtsProvider`); this test wraps the real
//! Sherpa provider with it, the way `VoiceAgent` does, and checks end to end that a
//! cancelled synthesis is followed by a fresh ONNX generate for the same text.
//!
//! Kept `#[ignore]` for default `cargo test` (needs Piper/VITS weights). CI runs it via
//! `bash scripts/ci/run-sherpa-example-ci.sh rust|e2e` after model download.
//! Shares process-wide `tts_generate_count` — run with `--test-threads=1`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::{TtsProgressiveSink, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::tts_cache::{CachingTtsProvider, PhraseCache};
use node_webrtc_rust_vendor_sherpa_onnx::{tts_generate_count, SherpaFactory};
use tokio::sync::mpsc;

/// The real Sherpa provider behind a private phrase cache.
fn cached_tts(model_path: &str, project_id: &str) -> CachingTtsProvider {
    let config = tts_config(model_path.to_string());
    let inner = SherpaFactory.create_tts(&config).expect("create TTS");
    let cache = Arc::new(PhraseCache::new(64 * 1024 * 1024, 32 * 1024 * 1024));
    let tts = CachingTtsProvider::new(inner, config, cache);
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some(project_id.into()),
        ..Default::default()
    });
    tts
}

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
    }
}

const FOUR_SENTENCES: &str = "The first sentence is about the weather today. \
The second sentence talks about the morning trains. \
The third sentence mentions a small cafe near the station. \
The fourth sentence closes the reply politely.";

fn total_pcm_bytes(chunks: &[node_webrtc_rust_speech::pipeline::TtsAudioChunk]) -> usize {
    chunks.iter().map(|chunk| chunk.pcm.len()).sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn cancelled_synthesis_is_not_cached() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");

    // (a) Reference: full uncancelled synthesis in its own cache.
    let ref_tts = cached_tts(&model_path, "ref-b2");
    let ref_chunks = ref_tts
        .synthesize(FOUR_SENTENCES)
        .await
        .expect("reference synth");
    let ref_bytes = total_pcm_bytes(&ref_chunks);
    assert!(ref_bytes > 0, "reference synthesis produced no audio");

    // (b) Cancelled: the receiver flips `cancel` on the first non-empty chunk.
    let tts = cached_tts(&model_path, "p1-b2");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::clone(&cancel),
    };
    let cancel_w = Arc::clone(&cancel);
    let recv = tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            if !chunk.pcm.is_empty() {
                cancel_w.store(true, Ordering::SeqCst);
            }
        }
    });
    // Ok or Err are both acceptable: the point is what ends up in the cache.
    let _ = tts.synthesize_progressive(FOUR_SENTENCES, Some(sink)).await;
    let _ = recv.await;
    assert!(
        cancel.load(Ordering::SeqCst),
        "the sink never received a chunk, so the synthesis was not cancelled mid-way"
    );

    // (c) Replay on the same provider (same project scope, same cache key).
    let gen_before = tts_generate_count();
    let replay_chunks = tts.synthesize(FOUR_SENTENCES).await.expect("replay synth");
    let gen_after = tts_generate_count();
    let replay_bytes = total_pcm_bytes(&replay_chunks);
    let cache_hit = gen_after == gen_before;

    assert!(
        gen_after > gen_before,
        "replay after a cancelled synthesis was served from the cache \
         (generate count {gen_before} -> {gen_after})"
    );
    assert!(
        replay_bytes as f64 >= 0.85 * ref_bytes as f64,
        "replay after a cancelled synthesis is truncated: replay_bytes={replay_bytes} \
         ref_bytes={ref_bytes} cache_hit={cache_hit} (generate count {gen_before} -> {gen_after})"
    );
}
