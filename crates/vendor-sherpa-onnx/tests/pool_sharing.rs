//! Integration tests for Sherpa model pooling (require downloaded weights).

use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::{
    reset_create_counters, stt_recognizer_create_count, tts_engine_create_count, SherpaFactory,
    SherpaModelPool,
};

fn stt_config(model_path: String) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        language: Some("en".into()),
        api_key: None,
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
#[ignore = "requires SHERPA_STT_MODEL_PATH with valid Zipformer bundle"]
async fn two_stt_sessions_share_one_recognizer() {
    let model_path =
        std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");

    reset_create_counters();
    let pool = SherpaModelPool::global();
    let before_entries = pool.stt_entry_count();

    let factory = SherpaFactory;
    let config = stt_config(model_path.clone());

    let mut first = factory.create_stt(&config).expect("stt a");
    let mut second = factory.create_stt(&config).expect("stt b");

    first.start().await.expect("start a");
    second.start().await.expect("start b");

    assert_eq!(
        stt_recognizer_create_count(),
        1,
        "expected a single OnlineRecognizer::create for the same model path"
    );
    assert_eq!(pool.stt_entry_count(), before_entries + 1);

    let ptr_a = pool.shared_stt_ptr(&config).expect("pooled stt");
    assert_eq!(ptr_a, pool.shared_stt_ptr(&config).expect("same ptr"));

    first.stop().await.expect("stop a");
    second.stop().await.expect("stop b");

    assert_eq!(
        pool.stt_entry_count(),
        before_entries + 1,
        "pool keeps recognizer warm after sessions stop"
    );
}

#[tokio::test]
#[ignore = "requires SHERPA_STT_MODEL_PATH with valid Zipformer bundle"]
async fn stt_stop_on_one_session_does_not_break_sibling() {
    let model_path =
        std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");

    let factory = SherpaFactory;
    let config = stt_config(model_path);

    let mut first = factory.create_stt(&config).expect("stt a");
    let mut second = factory.create_stt(&config).expect("stt b");

    first.start().await.expect("start a");
    second.start().await.expect("start b");

    first.stop().await.expect("stop a");
    second.stop().await.expect("stop b");

    first.start().await.expect("restart a");
    second.start().await.expect("restart b");

    first.stop().await.expect("stop a again");
    second.stop().await.expect("stop b again");
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn two_tts_providers_share_one_engine() {
    let model_path =
        std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");

    reset_create_counters();
    let pool = SherpaModelPool::global();
    let before_entries = pool.tts_entry_count();

    let factory = SherpaFactory;
    let config = tts_config(model_path);

    let tts_a = factory.create_tts(&config).expect("tts a");
    let tts_b = factory.create_tts(&config).expect("tts b");

    tts_a
        .synthesize("Hello from provider A.")
        .await
        .expect("synthesize a");
    tts_b
        .synthesize("Hello from provider B.")
        .await
        .expect("synthesize b");

    assert_eq!(
        tts_engine_create_count(),
        1,
        "expected a single OfflineTts::create for the same model path"
    );
    assert_eq!(pool.tts_entry_count(), before_entries + 1);
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn tts_different_speakers_share_engine_same_model_dir() {
    let model_path =
        std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");

    reset_create_counters();

    let factory = SherpaFactory;
    let mut config_a = tts_config(model_path.clone());
    config_a.voice = Some("0".into());
    let mut config_b = tts_config(model_path);
    config_b.voice = Some("1".into());

    let tts_a = factory.create_tts(&config_a).expect("tts a");
    let tts_b = factory.create_tts(&config_b).expect("tts b");

    tts_a.synthesize("Speaker zero.").await.expect("a");
    tts_b.synthesize("Speaker one.").await.expect("b");

    assert_eq!(
        tts_engine_create_count(),
        1,
        "speaker id is a generation param; pool key is model directory only"
    );
}
