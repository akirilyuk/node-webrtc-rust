use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{FinalizeMode, VendorFactory};
use node_webrtc_rust_vendor_fishaudio::FishAudioFactory;

fn config(model: Option<&str>) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Fishaudio,
        model: model.map(str::to_string),
        model_path: None,
        voice: Some("default".into()),
        api_key: Some("test-key".into()),
        endpoint: None,
        sentence_case: Default::default(),
    }
}

#[test]
fn fishaudio_factory_creates_tts_with_drama_3() {
    let factory = FishAudioFactory;
    let tts = factory
        .create_tts(&config(None))
        .unwrap_or_else(|_| panic!("default model should be accepted"));
    assert_eq!(tts.vendor_name(), "fishaudio");
    assert!(factory.create_tts(&config(Some("drama-3"))).is_ok());
}

#[test]
fn fishaudio_factory_rejects_unknown_model() {
    let factory = FishAudioFactory;
    for model in ["drama3", "s1", "s1-mini"] {
        match factory.create_tts(&config(Some(model))) {
            Err(SpeechError::Config(message)) => assert!(message.contains(model), "{message}"),
            Err(other) => panic!("expected Config error, got {other}"),
            Ok(_) => panic!("model {model} must be rejected"),
        }
    }
}

fn stt_config(model: Option<&str>) -> SttConfig {
    SttConfig {
        provider: SttVendor::Fishaudio,
        model: model.map(str::to_string),
        model_path: None,
        language: None,
        api_key: Some("test-key".into()),
        endpoint: None,
    }
}

#[test]
fn fishaudio_factory_creates_stt_with_transcribe_1() {
    let factory = FishAudioFactory;
    for model in [None, Some("transcribe-1")] {
        let stt = factory
            .create_stt(&stt_config(model))
            .unwrap_or_else(|_| panic!("transcribe-1 should be accepted"));
        assert_eq!(stt.vendor_name(), "fishaudio");
        assert_eq!(stt.finalize_mode(), FinalizeMode::Deferred);
    }
}

#[test]
fn fishaudio_factory_rejects_unknown_stt_model() {
    let factory = FishAudioFactory;
    for model in ["transcribe-1-pro", "transcribe1", "scribe_v2_realtime"] {
        match factory.create_stt(&stt_config(Some(model))) {
            Err(SpeechError::Config(message)) => assert!(message.contains(model), "{message}"),
            Err(other) => panic!("expected Config error, got {other}"),
            Ok(_) => panic!("STT model {model} must be rejected"),
        }
    }
}
