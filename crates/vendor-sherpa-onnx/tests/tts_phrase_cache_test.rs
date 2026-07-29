//! Sherpa TTS phrase cache behavior (model integration).
//!
//! Kept `#[ignore]` for default `cargo test` (needs Piper/VITS weights). CI runs them via
//! `bash scripts/ci/run-sherpa-example-ci.sh rust|e2e` after model download.
//! Share process-wide `tts_generate_count` — run with `--test-threads=1`.

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::{
    reset_tts_generate_count, tts_generate_count, SherpaFactory,
};

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
    }
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn repeated_phrase_uses_single_onnx_generate_with_cache_enabled() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_on = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "1");

    reset_tts_generate_count();
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(model_path))
        .expect("factory should create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-cache".into()),
        ..Default::default()
    });

    let phrase = "Phrase cache integration test.";
    tts.synthesize(phrase).await.expect("first synth");
    tts.synthesize(phrase).await.expect("second synth");
    assert_eq!(
        tts_generate_count(),
        1,
        "cache hit should skip second OfflineTts::generate"
    );
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn different_phrases_use_two_onnx_generates_with_cache_enabled() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_on = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "1");

    reset_tts_generate_count();
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(model_path))
        .expect("factory should create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-cache".into()),
        ..Default::default()
    });

    tts.synthesize("First phrase.").await.expect("first synth");
    tts.synthesize("Second phrase.")
        .await
        .expect("second synth");
    assert_eq!(tts_generate_count(), 2);
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn cache_disabled_runs_generate_for_each_phrase() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_off = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "0");

    reset_tts_generate_count();
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(model_path))
        .expect("factory should create TTS");

    let phrase = "Cache disabled integration test.";
    tts.synthesize(phrase).await.expect("first synth");
    tts.synthesize(phrase).await.expect("second synth");
    assert_eq!(tts_generate_count(), 2);
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn different_project_ids_do_not_share_cached_phrase() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_on = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "1");

    reset_tts_generate_count();
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(model_path))
        .expect("factory should create TTS");

    let phrase = "Project scoped phrase.";
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-a".into()),
        ..Default::default()
    });
    tts.synthesize(phrase).await.expect("proj-a synth");

    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-b".into()),
        ..Default::default()
    });
    tts.synthesize(phrase).await.expect("proj-b synth");

    assert_eq!(
        tts_generate_count(),
        2,
        "different project_id must not reuse cached PCM"
    );
}
