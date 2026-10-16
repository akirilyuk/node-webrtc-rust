use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::VendorFactory;
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

#[test]
fn fishaudio_factory_has_no_stt() {
    use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
    let factory = FishAudioFactory;
    let result = factory.create_stt(&SttConfig {
        provider: SttVendor::Mock,
        model: None,
        model_path: None,
        language: None,
        api_key: None,
        endpoint: None,
    });
    assert!(matches!(result, Err(SpeechError::Config(_))));
}
