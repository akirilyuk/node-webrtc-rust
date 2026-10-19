#![cfg(feature = "live")]
//! The pooled client: two requests on one provider reuse one TCP connection, `last_transport`
//! reports how the call reached the server, and a server that never answers is bounded.

use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider};
use node_webrtc_rust_vendor_azure::{AzureStt, AzureTts, HTTP_TOTAL_TIMEOUT};
use node_webrtc_rust_vendor_live_support::http_mock::{pcm_16bit_mono, MockHttp, Reply};

fn tts() -> AzureTts {
    AzureTts::new(&TtsConfig {
        provider: TtsVendor::Azure,
        model: None,
        model_path: None,
        voice: None,
        api_key: Some("k".into()),
        endpoint: Some("eastus.tts.speech.microsoft.com".into()),
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap()
}

fn stt() -> AzureStt {
    AzureStt::new(&SttConfig {
        provider: SttVendor::Azure,
        model: None,
        model_path: None,
        language: Some("en-US".into()),
        api_key: Some("k".into()),
        endpoint: Some("res.cognitiveservices.azure.com".into()),
        transport: None,
    })
    .unwrap()
}

async fn stt_utterance(stt: &mut AzureStt) -> Result<Option<SttTranscript>, String> {
    stt.start().await.map_err(|e| e.to_string())?;
    stt.push_audio(Bytes::from(pcm_16bit_mono(16_000, 1000)))
        .await
        .map_err(|e| e.to_string())?;
    stt.finalize_utterance().await.map_err(|e| e.to_string())?;
    stt.poll_transcript().await.map_err(|e| e.to_string())
}

#[tokio::test]
async fn tts_two_requests_on_one_provider_reuse_one_connection() {
    let mock = MockHttp::start(Reply::ok("audio/basic", pcm_16bit_mono(48_000, 200))).await;
    let provider = tts().__with_url_override(format!("{}/cognitiveservices/v1", mock.base_url()));
    assert_eq!(provider.last_transport(), None);
    for _ in 0..2 {
        let chunks = provider.synthesize("hello").await.unwrap();
        assert!(!chunks.is_empty());
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn stt_two_requests_on_one_provider_reuse_one_connection() {
    let body = br#"{"RecognitionStatus":"Success","DisplayText":"Hello."}"#.to_vec();
    let mock = MockHttp::start(Reply::ok("application/json", body)).await;
    let mut provider = stt().__with_url_override(format!("{}/stt", mock.base_url()));
    for _ in 0..2 {
        let got = stt_utterance(&mut provider).await.unwrap();
        assert!(matches!(got, Some(SttTranscript::Final(ref t)) if t == "Hello."));
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn tts_server_that_never_answers_errors_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let provider = tts()
        .__with_url_override(format!("{}/cognitiveservices/v1", mock.base_url()))
        .__with_total_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), provider.synthesize("hello"))
        .await
        .expect("the client timeout fires before the test guard");
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn stt_server_that_never_answers_errors_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let mut provider = stt()
        .__with_url_override(format!("{}/stt", mock.base_url()))
        .__with_total_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), stt_utterance(&mut provider))
        .await
        .expect("the client timeout fires before the test guard");
    // A request that timed out is an error from the poll (`rt.silent-failure`), never silence.
    assert!(result.is_err(), "expected an Err, got {result:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn default_total_timeout_is_bounded() {
    assert_eq!(HTTP_TOTAL_TIMEOUT, Duration::from_secs(60));
}
