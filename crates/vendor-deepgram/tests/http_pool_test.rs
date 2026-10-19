#![cfg(feature = "live")]
//! The pooled REST client: two requests on one provider reuse one TCP connection and
//! `last_transport` reports how the call reached the server.

use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;
use node_webrtc_rust_vendor_live_support::http_mock::{pcm_16bit_mono, MockHttp, Reply};

#[tokio::test]
async fn rest_two_requests_on_one_provider_reuse_one_connection() {
    let mock = MockHttp::start(Reply::ok(
        "application/octet-stream",
        pcm_16bit_mono(48_000, 100),
    ))
    .await;
    let provider = DeepgramFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Deepgram,
            model: None,
            model_path: None,
            voice: None,
            api_key: Some("k".into()),
            endpoint: Some(mock.base_url()),
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::Http),
        })
        .unwrap();
    assert_eq!(provider.last_transport(), None);
    for _ in 0..2 {
        assert!(!provider.synthesize("hello").await.unwrap().is_empty());
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}
