//! B2: a cancelled progressive synthesis must not be stored in the phrase cache.
//!
//! Kept `#[ignore]` for default `cargo test` (needs Piper/VITS weights). CI runs it via
//! `bash scripts/ci/run-sherpa-example-ci.sh rust|e2e` after model download.
//! Shares process-wide `tts_generate_count` — run with `--test-threads=1`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::{TtsProgressiveSink, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{tts_generate_count, SherpaFactory};
use tokio::sync::mpsc;

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
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
    let _cache_on = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "1");
    let factory = SherpaFactory;

    // (a) Reference: full uncancelled synthesis in its own cache scope.
    let ref_tts = factory
        .create_tts(&tts_config(model_path.clone()))
        .expect("create reference TTS");
    ref_tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("ref-b2".into()),
        ..Default::default()
    });
    let ref_chunks = ref_tts
        .synthesize(FOUR_SENTENCES)
        .await
        .expect("reference synth");
    let ref_bytes = total_pcm_bytes(&ref_chunks);
    assert!(ref_bytes > 0, "reference synthesis produced no audio");

    // (b) Cancelled: the receiver flips `cancel` on the first non-empty chunk.
    let tts = factory
        .create_tts(&tts_config(model_path))
        .expect("create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("p1-b2".into()),
        ..Default::default()
    });
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
        replay_bytes as f64 >= 0.85 * ref_bytes as f64,
        "replay after a cancelled synthesis is truncated: replay_bytes={replay_bytes} \
         ref_bytes={ref_bytes} cache_hit={cache_hit} (generate count {gen_before} -> {gen_after})"
    );
}
