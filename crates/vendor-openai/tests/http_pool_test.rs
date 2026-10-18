#![cfg(feature = "live")]
//! The pooled client: two requests on one provider reuse one TCP connection, `last_transport`
//! reports how the call reached the server, and a server that never answers is bounded.

use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider};
use node_webrtc_rust_vendor_live_support::http_mock::{pcm_16bit_mono, MockHttp, Reply};
use node_webrtc_rust_vendor_live_support::tts::assert_chunks_match_source;
use node_webrtc_rust_vendor_openai::{OpenAiStt, OpenAiTts, HTTP_TOTAL_TIMEOUT};

fn tts(base: &str) -> OpenAiTts {
    OpenAiTts::new(&TtsConfig {
        provider: TtsVendor::Openai,
        model: Some("tts-1".into()),
        model_path: None,
        voice: Some("alloy".into()),
        api_key: Some("k".into()),
        endpoint: Some(format!("{base}/v1")),
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap()
}

fn stt(base: &str, model: &str) -> OpenAiStt {
    OpenAiStt::new(&SttConfig {
        provider: SttVendor::Openai,
        model: Some(model.into()),
        model_path: None,
        language: Some("en".into()),
        api_key: Some("k".into()),
        endpoint: Some(format!("{base}/v1")),
        transport: None,
    })
    .unwrap()
}

async fn stt_utterance(stt: &mut OpenAiStt) -> Result<Vec<SttTranscript>, String> {
    stt.start().await.map_err(|e| e.to_string())?;
    stt.push_audio(Bytes::from(pcm_16bit_mono(16_000, 1000)))
        .await
        .map_err(|e| e.to_string())?;
    stt.finalize_utterance().await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(t) = stt.poll_transcript().await.map_err(|e| e.to_string())? {
        let done = matches!(t, SttTranscript::Final(_));
        out.push(t);
        if done {
            break;
        }
    }
    Ok(out)
}

const SSE: &str = concat!(
    "data: {\"type\":\"transcript.text.delta\",\"delta\":\"Hello.\"}\n\n",
    "data: {\"type\":\"transcript.text.done\",\"text\":\"Hello.\"}\n\n"
);

#[tokio::test]
async fn tts_stream_two_requests_on_one_provider_reuse_one_connection() {
    let mock = MockHttp::start(Reply::ok("audio/pcm", pcm_16bit_mono(24_000, 200))).await;
    let provider = tts(&mock.base_url());
    assert_eq!(provider.last_transport(), None);
    for _ in 0..2 {
        let chunks = provider
            .synthesize_progressive("hello", None)
            .await
            .unwrap();
        assert!(!chunks.is_empty());
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn tts_full_body_two_requests_on_one_provider_reuse_one_connection() {
    let mock = MockHttp::start(Reply::ok("audio/pcm", pcm_16bit_mono(24_000, 200))).await;
    let provider = tts(&mock.base_url());
    for _ in 0..2 {
        assert!(!provider.synthesize("hello").await.unwrap().is_empty());
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("sdk"));
}

#[tokio::test]
async fn stt_file_json_two_requests_on_one_provider_reuse_one_connection() {
    let mock = MockHttp::start(Reply::ok(
        "application/json",
        br#"{"text":"Hello."}"#.to_vec(),
    ))
    .await;
    let mut provider = stt(&mock.base_url(), "whisper-1");
    for _ in 0..2 {
        let got = stt_utterance(&mut provider).await.unwrap();
        assert_eq!(got, vec![SttTranscript::Final("Hello.".into())]);
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("sdk"));
}

#[tokio::test]
async fn stt_file_sse_two_requests_on_one_provider_reuse_one_connection() {
    let mock = MockHttp::start(Reply::ok("text/event-stream", SSE.as_bytes().to_vec())).await;
    let mut provider = stt(&mock.base_url(), "gpt-4o-mini-transcribe");
    for _ in 0..2 {
        let got = stt_utterance(&mut provider).await.unwrap();
        assert_eq!(got.last(), Some(&SttTranscript::Final("Hello.".into())));
    }
    assert_eq!(mock.requests(), 2);
    assert_eq!(mock.accepted(), 1);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn tts_server_that_never_answers_errors_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let provider = tts(&mock.base_url()).__with_total_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        provider.synthesize_progressive("hello", None),
    )
    .await
    .expect("the client timeout fires before the test guard");
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn stt_server_that_never_answers_ends_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let mut provider = stt(&mock.base_url(), "gpt-4o-mini-transcribe")
        .__with_total_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), stt_utterance(&mut provider))
        .await
        .expect("the client timeout fires before the test guard");
    // The poll ends (error or no transcript) once the client timeout fires; it never yields a final.
    assert!(
        !matches!(&result, Ok(v) if !v.is_empty()),
        "unexpected transcript: {result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn default_total_timeout_is_bounded() {
    assert_eq!(HTTP_TOTAL_TIMEOUT, Duration::from_secs(60));
}

/// Catalog `audio.sample-rate-mismatch`: 1 s of 24 kHz mono `pcm` is 1000 ms and 192000 bytes of
/// 48 kHz stereo.
#[tokio::test]
async fn one_second_of_24k_pcm_is_one_second_of_48k_stereo() {
    let mock = MockHttp::start(Reply::ok("audio/pcm", pcm_16bit_mono(24_000, 1000))).await;
    let chunks = tts(&mock.base_url()).synthesize("hello").await.unwrap();
    assert_chunks_match_source("openai", &chunks, 1000);
    let bytes: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(bytes, 192_000);
}
