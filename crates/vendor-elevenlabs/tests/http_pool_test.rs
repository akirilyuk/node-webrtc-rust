#![cfg(feature = "live")]
//! The pooled HTTP client: two requests on one provider reuse one TCP connection and
//! `last_transport` reports how the call reached the server.

use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use node_webrtc_rust_vendor_live_support::http_mock::{pcm_16bit_mono, MockHttp, Reply};

async fn two_requests_reuse_one_connection(model: &str) {
    let mock = MockHttp::start(Reply::ok("audio/pcm", pcm_16bit_mono(48_000, 100))).await;
    let provider = ElevenLabsFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
            model: Some(model.into()),
            model_path: None,
            voice: Some("voice-id".into()),
            api_key: Some("k".into()),
            endpoint: Some(mock.base_url()),
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::Http),
        })
        .unwrap();
    assert_eq!(provider.last_transport(), None);
    for _ in 0..2 {
        assert!(!provider
            .synthesize_progressive("hello", None)
            .await
            .unwrap()
            .is_empty());
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn http_stream_two_requests_on_one_provider_reuse_one_connection() {
    two_requests_reuse_one_connection("eleven_multilingual_v2").await;
}

#[tokio::test]
async fn full_body_two_requests_on_one_provider_reuse_one_connection() {
    two_requests_reuse_one_connection("eleven_v3").await;
}
