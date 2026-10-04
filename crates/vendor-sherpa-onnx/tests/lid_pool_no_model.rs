//! LID pool behaviour that does not need Whisper weights.

use node_webrtc_rust_speech::config::LanguageIdConfig;
use node_webrtc_rust_vendor_sherpa_onnx::{preload_language_id, SherpaLanguageId, SherpaModelPool};

fn missing_dir_config() -> LanguageIdConfig {
    LanguageIdConfig {
        enabled: Some(true),
        model_path: Some("/nonexistent/nwr-lid-model-dir".into()),
        allowlist: None,
        min_speech_ms: None,
        continuous: None,
        lid_max_clip_ms: None,
        lid_gate_max_wait_ms: None,
        timing: None,
        tts_exclusion: None,
    }
}

#[test]
fn failed_preload_leaves_pool_empty_and_retryable() {
    let pool = SherpaModelPool::global();
    let before = pool.lid_entry_count();
    let config = missing_dir_config();

    assert!(preload_language_id(&config).is_err());

    // Constructing providers starts background preloads; failures are logged, never panic,
    // and must not wedge the in-flight set (a second construct still runs and fails again).
    let _first = SherpaLanguageId::new(&config);
    pool.join_lid_preloads();
    let _second = SherpaLanguageId::new(&config);
    pool.join_lid_preloads();

    assert_eq!(pool.lid_entry_count(), before);
    assert!(pool.shared_lid_ptr(&config).is_none());
}
