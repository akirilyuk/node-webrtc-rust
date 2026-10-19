#![cfg(feature = "live")]
//! Azure TTS body streaming and STT failures against a local server (no key, no network).
//!
//! Every chunk reaches the sink once, a body error after audio is an error without a retry, and
//! a failed recognition request is an `Err` from the poll, never silence
//! (`contract.duplicate-emit`, `rt.silent-failure`).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    SttProvider, SttTranscript, TtsAudioChunk, TtsProgressiveSink, TtsProvider,
};
use node_webrtc_rust_vendor_azure::{AzureStt, AzureTts};
use node_webrtc_rust_vendor_live_support::http_mock::{
    chunked_head, pcm_16bit_mono, script, write_chunk, MockHttp, Reply,
};
use node_webrtc_rust_vendor_live_support::tts::assert_chunks_match_source;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

const BOUND: Duration = Duration::from_secs(10);
/// 20 ms of 48 kHz mono s16le.
const CHUNK_BYTES: usize = 1920;

fn tts(mock: &MockHttp) -> AzureTts {
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
    .__with_url_override(format!("{}/cognitiveservices/v1", mock.base_url()))
}

fn stt(mock: &MockHttp) -> AzureStt {
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
    .__with_url_override(format!("{}/stt", mock.base_url()))
}

struct Rig {
    sink: TtsProgressiveSink,
    rx: mpsc::UnboundedReceiver<TtsAudioChunk>,
}

fn rig() -> Rig {
    let (tx, rx) = mpsc::unbounded_channel();
    Rig {
        sink: TtsProgressiveSink {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
        },
        rx,
    }
}

fn drain(rx: &mut mpsc::UnboundedReceiver<TtsAudioChunk>) -> Vec<TtsAudioChunk> {
    let mut out = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        out.push(chunk);
    }
    out
}

fn bytes_of(chunks: &[TtsAudioChunk]) -> usize {
    chunks.iter().map(|c| c.pcm.len()).sum()
}

#[tokio::test]
async fn request_asks_for_raw_48khz_pcm_with_the_documented_headers() {
    let mock = MockHttp::start(Reply::ok("audio/basic", pcm_16bit_mono(48_000, 100))).await;
    tts(&mock).synthesize("a < b & c").await.unwrap();
    let head = mock.heads().remove(0).to_ascii_lowercase();
    assert!(
        head.contains("x-microsoft-outputformat: raw-48khz-16bit-mono-pcm"),
        "{head}"
    );
    assert!(
        head.contains("content-type: application/ssml+xml"),
        "{head}"
    );
    assert!(head.contains("user-agent: "), "{head}");
}

#[tokio::test]
async fn body_reaches_the_sink_before_the_response_ends() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let go_rx = Arc::new(Mutex::new(Some(go_rx)));
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let go_rx = go_rx.lock().unwrap().take();
        async move {
            let _ = stream.write_all(&chunked_head("audio/basic")).await;
            write_chunk(&mut stream, &[1u8; CHUNK_BYTES]).await;
            // The second chunk is released only after the client showed the first to the sink.
            if let Some(go_rx) = go_rx {
                let _ = go_rx.await;
            }
            write_chunk(&mut stream, &[2u8; CHUNK_BYTES]).await;
            let _ = stream.write_all(b"0\r\n\r\n").await;
        }
    })))
    .await;
    let provider = tts(&mock);
    let Rig { sink, mut rx } = rig();

    let call =
        tokio::spawn(async move { provider.synthesize_progressive("hello", Some(sink)).await });
    let first = tokio::time::timeout(BOUND, rx.recv())
        .await
        .expect("no chunk before the body ended")
        .expect("first chunk");
    assert_eq!(first.pcm[0], 1);
    assert_eq!(first.duration_ms, 20);
    assert!(!call.is_finished(), "the call must still be running");
    go_tx.send(()).unwrap();

    let chunks = tokio::time::timeout(BOUND, call)
        .await
        .expect("call timed out")
        .unwrap()
        .expect("synthesize failed");
    let second = rx.recv().await.expect("second chunk");
    assert_eq!(second.pcm[0], 2);
    assert!(rx.try_recv().is_err(), "each chunk exactly once");
    assert_eq!(bytes_of(&chunks), 2 * CHUNK_BYTES * 2);
    assert_eq!(mock.requests(), 1);
}

#[tokio::test]
async fn odd_chunk_sizes_stay_sample_aligned() {
    let pcm: Vec<u8> = (0..CHUNK_BYTES).map(|i| (i % 251) as u8).collect();
    let body = pcm.clone();
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let body = body.clone();
        async move {
            let _ = stream.write_all(&chunked_head("audio/basic")).await;
            for part in [&body[..21], &body[21..25], &body[25..1001], &body[1001..]] {
                write_chunk(&mut stream, part).await;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        }
    })))
    .await;
    let Rig { sink, mut rx } = rig();
    let returned = tts(&mock)
        .synthesize_progressive("hello", Some(sink))
        .await
        .unwrap();
    let sunk = drain(&mut rx);
    assert_eq!(bytes_of(&sunk), pcm.len() * 2);
    assert_eq!(bytes_of(&returned), bytes_of(&sunk), "one send path");
    assert!(sunk.iter().all(|c| c.pcm.len() % 4 == 0), "sample aligned");
    let stereo: Vec<u8> = sunk.iter().flat_map(|c| c.pcm.to_vec()).collect();
    let left: Vec<u8> = stereo
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|s| [s[0], s[1]])
        .collect();
    assert_eq!(left, pcm);
}

#[tokio::test]
async fn body_error_after_audio_is_an_error_without_a_retry() {
    let mock = MockHttp::start(Reply::Script(script(|mut stream| async move {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/basic\r\nContent-Length: {}\r\n\r\n",
            CHUNK_BYTES * 10
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&[1u8; CHUNK_BYTES]).await;
        let _ = stream.flush().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(stream);
    })))
    .await;
    let provider = tts(&mock);
    let mut rig = rig();
    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("call timed out");
    assert!(
        matches!(result, Err(SpeechError::Vendor { .. })),
        "a body that ends early is an error: {result:?}"
    );
    assert_eq!(
        drain(&mut rig.rx).len(),
        1,
        "the audio so far reached the sink once"
    );
    assert_eq!(mock.requests(), 1, "no retry after audio");
}

#[tokio::test]
async fn an_empty_body_is_an_error() {
    let mock = MockHttp::start(Reply::ok("audio/basic", Vec::new())).await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "Azure TTS returned no audio")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_4xx_carries_the_status_and_the_vendors_text() {
    let mock = MockHttp::start(Reply::Fixed {
        status: 401,
        content_type: "text/plain",
        body: b"Access denied due to invalid subscription key.".to_vec(),
    })
    .await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("401"), "{message}");
            assert!(message.contains("invalid subscription key"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn no_audio_within_the_first_audio_bound_is_an_error() {
    let mock = MockHttp::start(Reply::Hang).await;
    let provider = tts(&mock).__with_bounds(Duration::from_millis(300), Duration::from_secs(5));
    let result = tokio::time::timeout(BOUND, provider.synthesize("hello"))
        .await
        .expect("the first-audio bound fires before the test guard");
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("first audio timed out"), "{message}")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_body_that_stalls_after_audio_is_an_error() {
    let mock = MockHttp::start(Reply::Script(script(|mut stream| async move {
        let _ = stream.write_all(&chunked_head("audio/basic")).await;
        write_chunk(&mut stream, &[1u8; CHUNK_BYTES]).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    })))
    .await;
    let provider = tts(&mock).__with_bounds(Duration::from_secs(5), Duration::from_millis(300));
    let mut rig = rig();
    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("the idle bound fires before the test guard");
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("stalled"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(drain(&mut rig.rx).len(), 1);
}

// ---- STT: a failed request is an error ----

async fn utterance(stt: &mut AzureStt) -> Result<Option<SttTranscript>, SpeechError> {
    stt.start().await?;
    stt.push_audio(Bytes::from(pcm_16bit_mono(16_000, 1000)))
        .await?;
    stt.finalize_utterance().await?;
    stt.poll_transcript().await
}

#[tokio::test]
async fn stt_http_500_is_an_error_with_the_status() {
    let mock = MockHttp::start(Reply::Fixed {
        status: 500,
        content_type: "text/plain",
        body: b"service unavailable text".to_vec(),
    })
    .await;
    let mut provider = stt(&mock);
    match utterance(&mut provider).await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("500"), "{message}");
            assert!(message.contains("service unavailable text"), "{message}");
        }
        other => panic!("expected an Err, got {other:?}"),
    }
}

#[tokio::test]
async fn stt_server_that_never_answers_is_an_error_within_the_bound() {
    let mock = MockHttp::start(Reply::Hang).await;
    let mut provider = stt(&mock).__with_total_timeout(Duration::from_millis(300));
    let result = tokio::time::timeout(BOUND, utterance(&mut provider))
        .await
        .expect("the client timeout fires before the test guard");
    assert!(
        matches!(result, Err(SpeechError::Vendor { .. })),
        "expected an Err, got {result:?}"
    );
}

#[tokio::test]
async fn stt_recognition_status_error_is_an_error() {
    let mock = MockHttp::start(Reply::ok(
        "application/json",
        br#"{"RecognitionStatus":"Error"}"#.to_vec(),
    ))
    .await;
    let mut provider = stt(&mock);
    match utterance(&mut provider).await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("RecognitionStatus Error"), "{message}")
        }
        other => panic!("expected an Err, got {other:?}"),
    }
}

#[tokio::test]
async fn stt_answer_that_is_not_the_documented_json_is_an_error() {
    let mock = MockHttp::start(Reply::ok("text/html", b"<html>gateway</html>".to_vec())).await;
    let mut provider = stt(&mock);
    assert!(matches!(
        utterance(&mut provider).await,
        Err(SpeechError::Vendor { .. })
    ));
}

/// `NoMatch`, `InitialSilenceTimeout` and `BabbleTimeout` are documented outcomes for audio with
/// nothing to transcribe: no final, no error.
#[tokio::test]
async fn stt_no_speech_statuses_are_no_final_and_no_error() {
    for status in ["NoMatch", "InitialSilenceTimeout", "BabbleTimeout"] {
        let body = format!(r#"{{"RecognitionStatus":"{status}"}}"#).into_bytes();
        let mock = MockHttp::start(Reply::ok("application/json", body)).await;
        let mut provider = stt(&mock);
        assert!(
            matches!(utterance(&mut provider).await, Ok(None)),
            "{status}"
        );
    }
}

/// Catalog `audio.sample-rate-mismatch`: 1 s of `raw-48khz-16bit-mono-pcm` is 1000 ms and
/// 192000 bytes of 48 kHz stereo.
#[tokio::test]
async fn one_second_of_48k_pcm_is_one_second_of_48k_stereo() {
    let mock = MockHttp::start(Reply::ok("audio/basic", pcm_16bit_mono(48_000, 1000))).await;
    let chunks = tts(&mock).synthesize("hello").await.unwrap();
    assert_chunks_match_source("azure", &chunks, 1000);
    assert_eq!(bytes_of(&chunks), 192_000);
}
