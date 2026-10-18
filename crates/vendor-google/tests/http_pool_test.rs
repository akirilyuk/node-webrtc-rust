#![cfg(feature = "live")]
//! The pooled REST client: two requests on one provider reuse one TCP connection,
//! `last_transport` reports how the call reached the server, and a server that never answers is
//! bounded. Covers the REST paths (Neural2 `text:synthesize`, V1 `speech:recognize`).

use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider};
use node_webrtc_rust_vendor_google::{GoogleStt, GoogleTts, HTTP_TOTAL_TIMEOUT};
use node_webrtc_rust_vendor_live_support::http_mock::{pcm_16bit_mono, MockHttp, Reply};

fn tts(url: &str) -> GoogleTts {
    GoogleTts::new(&TtsConfig {
        provider: TtsVendor::Google,
        model: None,
        model_path: None,
        voice: Some("en-US-Neural2-A".into()),
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap()
    .__with_rest_url_override(format!("{url}/v1/text:synthesize"))
}

fn stt(url: &str) -> GoogleStt {
    GoogleStt::new(&SttConfig {
        provider: SttVendor::Google,
        model: Some("latest_long".into()),
        model_path: None,
        language: Some("en-US".into()),
        api_key: None,
        endpoint: None,
        transport: None,
    })
    .unwrap()
    .__with_rest_url_override(format!("{url}/v1/speech:recognize"))
}

async fn stt_utterance(stt: &mut GoogleStt) -> Result<Option<SttTranscript>, String> {
    stt.start().await.map_err(|e| e.to_string())?;
    stt.push_audio(Bytes::from(pcm_16bit_mono(16_000, 1000)))
        .await
        .map_err(|e| e.to_string())?;
    stt.finalize_utterance().await.map_err(|e| e.to_string())?;
    for _ in 0..250 {
        if let Some(t) = stt.poll_transcript().await.map_err(|e| e.to_string())? {
            return Ok(Some(t));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(None)
}

#[tokio::test]
async fn tts_rest_two_requests_on_one_provider_reuse_one_connection() {
    let body = format!(
        "{{\"audioContent\":\"{}\"}}",
        STANDARD.encode(pcm_16bit_mono(48_000, 200))
    );
    let mock = MockHttp::start(Reply::ok("application/json", body.into_bytes())).await;
    let provider = tts(&mock.base_url());
    assert_eq!(provider.last_transport(), None);
    for _ in 0..2 {
        assert!(!provider.synthesize("hello").await.unwrap().is_empty());
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn stt_v1_rest_two_requests_on_one_provider_reuse_one_connection() {
    let body = br#"{"results":[{"alternatives":[{"transcript":"Hello."}]}]}"#.to_vec();
    let mock = MockHttp::start(Reply::ok("application/json", body)).await;
    let mut provider = stt(&mock.base_url());
    for _ in 0..2 {
        let got = stt_utterance(&mut provider).await.unwrap();
        assert_eq!(got, Some(SttTranscript::Final("Hello.".into())));
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn tts_rest_server_that_never_answers_errors_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let provider = tts(&mock.base_url()).__with_total_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), provider.synthesize("hello"))
        .await
        .expect("the client timeout fires before the test guard");
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn stt_v1_rest_server_that_never_answers_is_dropped_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let mut provider = stt(&mock.base_url()).__with_total_timeout(Duration::from_millis(300));
    provider.start().await.unwrap();
    provider
        .push_audio(Bytes::from(pcm_16bit_mono(16_000, 1000)))
        .await
        .unwrap();
    provider.finalize_utterance().await.unwrap();
    // The request task swallows the error, so observe the client dropping the connection.
    let started = Instant::now();
    while mock.closed() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "client did not give up on a server that never answers"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(mock.requests(), 1);
    assert_eq!(provider.poll_transcript().await.unwrap(), None);
}

#[test]
fn default_total_timeout_is_bounded() {
    assert_eq!(HTTP_TOTAL_TIMEOUT, Duration::from_secs(60));
}
