#![cfg(feature = "live")]
//! Groq TTS body streaming and STT failures against a local server (no key, no network).
//!
//! Every chunk reaches the sink once, a body error after audio is an error without a retry, and
//! a failed transcription request is an `Err` from the poll, never silence
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
use node_webrtc_rust_vendor_groq::{GroqStt, GroqTts};
use node_webrtc_rust_vendor_live_support::http_mock::{
    chunked_head, pcm_16bit_mono, script, wav_16bit_mono, write_chunk, MockHttp, Reply,
};
use node_webrtc_rust_vendor_live_support::tts::assert_chunks_match_source;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

const BOUND: Duration = Duration::from_secs(10);
/// 20 ms of 48 kHz mono s16le.
const CHUNK_BYTES: usize = 1920;

fn tts(mock: &MockHttp) -> GroqTts {
    GroqTts::new(&TtsConfig {
        provider: TtsVendor::Groq,
        model: None,
        model_path: None,
        voice: None,
        api_key: Some("k".into()),
        endpoint: Some(format!("{}/openai/v1/audio/speech", mock.base_url())),
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap()
}

fn stt(mock: &MockHttp) -> GroqStt {
    GroqStt::new(&SttConfig {
        provider: SttVendor::Groq,
        model: None,
        model_path: None,
        language: None,
        api_key: Some("k".into()),
        endpoint: Some(format!(
            "{}/openai/v1/audio/transcriptions",
            mock.base_url()
        )),
        transport: None,
    })
    .unwrap()
}

/// The 44-byte header of a WAV at `rate`, then a body of `n` 20 ms chunks filled with `fill`.
fn header(rate: u32) -> Vec<u8> {
    wav_16bit_mono(rate, 0)[..44].to_vec()
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
async fn body_reaches_the_sink_before_the_response_ends() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let go_rx = Arc::new(Mutex::new(Some(go_rx)));
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let go_rx = go_rx.lock().unwrap().take();
        async move {
            let _ = stream.write_all(&chunked_head("audio/wav")).await;
            let mut first = header(48_000);
            first.extend_from_slice(&[1u8; CHUNK_BYTES]);
            write_chunk(&mut stream, &first).await;
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
    // 48 kHz mono doubled to stereo.
    assert_eq!(bytes_of(&chunks), 2 * CHUNK_BYTES * 2);
    assert_eq!(mock.requests(), 1);
}

#[tokio::test]
async fn header_and_samples_split_at_odd_byte_boundaries_stay_sample_aligned() {
    let mut body = header(48_000);
    let pcm: Vec<u8> = (0..2 * CHUNK_BYTES).map(|i| (i % 251) as u8).collect();
    body.extend_from_slice(&pcm);
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let body = body.clone();
        async move {
            let _ = stream.write_all(&chunked_head("audio/wav")).await;
            // Cuts inside the RIFF header, the fmt chunk, and at odd sample offsets.
            let cuts = [5usize, 17, 44, 45, 1001, 1002, body.len()];
            let mut at = 0;
            for cut in cuts {
                write_chunk(&mut stream, &body[at..cut]).await;
                at = cut;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        }
    })))
    .await;
    let provider = tts(&mock);
    let Rig { sink, mut rx } = rig();
    let returned = provider
        .synthesize_progressive("hello", Some(sink))
        .await
        .unwrap();
    let sunk = drain(&mut rx);
    assert_eq!(bytes_of(&sunk), pcm.len() * 2);
    assert_eq!(bytes_of(&returned), bytes_of(&sunk), "one send path");
    assert!(sunk.iter().all(|c| c.pcm.len() % 4 == 0), "sample aligned");
    // Every mono sample is duplicated into both channels, in order.
    let stereo: Vec<u8> = sunk.iter().flat_map(|c| c.pcm.to_vec()).collect();
    let left: Vec<u8> = stereo
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|s| [s[0], s[1]])
        .collect();
    assert_eq!(left, pcm);
}

/// Catalog `audio.sample-rate-mismatch`: 1 s of WAV at any documented rate is about 1000 ms and
/// 192000 bytes of 48 kHz stereo.
#[tokio::test]
async fn one_second_of_wav_is_one_second_of_48k_stereo_at_every_rate() {
    for rate in [8_000, 16_000, 22_050, 24_000, 44_100, 48_000] {
        let mock = MockHttp::start(Reply::ok("audio/wav", wav_16bit_mono(rate, 1000))).await;
        let chunks = tts(&mock).synthesize("hello").await.unwrap();
        assert_chunks_match_source(&format!("groq_{rate}"), &chunks, 1000);
        let bytes = bytes_of(&chunks);
        assert!(
            (191_000..=193_000).contains(&bytes),
            "{rate} Hz: {bytes} bytes"
        );
    }
}

#[tokio::test]
async fn a_sample_rate_outside_8k_to_48k_is_a_named_error() {
    let mock = MockHttp::start(Reply::ok("audio/wav", wav_16bit_mono(96_000, 100))).await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(
                message.contains("unsupported TTS sample rate 96000"),
                "{message}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn body_error_after_audio_is_an_error_without_a_retry() {
    let mock = MockHttp::start(Reply::Script(script(|mut stream| async move {
        // Promises more than it sends, then resets the connection.
        let mut first = header(48_000);
        first.extend_from_slice(&[1u8; CHUNK_BYTES]);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\n\r\n",
            first.len() + CHUNK_BYTES * 10
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&first).await;
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
async fn a_body_without_audio_is_an_error() {
    let mock = MockHttp::start(Reply::ok("audio/wav", header(48_000))).await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "Groq TTS returned no audio")
        }
        other => panic!("{other:?}"),
    }
    let mock = MockHttp::start(Reply::ok("audio/wav", header(48_000)[..30].to_vec())).await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "Groq TTS body ended before the WAV data chunk")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_non_wav_body_is_a_named_error() {
    let mock = MockHttp::start(Reply::ok(
        "application/json",
        br#"{"ok":true} ........"#.to_vec(),
    ))
    .await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "Groq TTS response is not a WAV file")
        }
        other => panic!("{other:?}"),
    }
}

/// The vendor's own error text (for example an Orpheus terms-of-use message) is in the error, the
/// key is not.
#[tokio::test]
async fn a_4xx_carries_the_status_and_the_vendors_text_but_not_the_key() {
    let mock = MockHttp::start(Reply::Fixed {
        status: 400,
        content_type: "application/json",
        body: br#"{"error":{"message":"model terms must be accepted"}}"#.to_vec(),
    })
    .await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("400"), "{message}");
            assert!(message.contains("terms must be accepted"), "{message}");
            assert!(!message.contains("Bearer"), "{message}");
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
        let _ = stream.write_all(&chunked_head("audio/wav")).await;
        let mut first = header(48_000);
        first.extend_from_slice(&[1u8; CHUNK_BYTES]);
        write_chunk(&mut stream, &first).await;
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

async fn utterance(stt: &mut GroqStt) -> Result<Option<SttTranscript>, SpeechError> {
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
        content_type: "application/json",
        body: br#"{"error":"upstream exploded"}"#.to_vec(),
    })
    .await;
    let mut provider = stt(&mock);
    match utterance(&mut provider).await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("500"), "{message}");
            assert!(message.contains("upstream exploded"), "{message}");
        }
        other => panic!("expected an Err, got {other:?}"),
    }
    assert_eq!(mock.requests(), 1);
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
async fn stt_answer_without_a_text_field_is_an_error() {
    let mock = MockHttp::start(Reply::ok("text/html", b"<html>gateway</html>".to_vec())).await;
    let mut provider = stt(&mock);
    assert!(matches!(
        utterance(&mut provider).await,
        Err(SpeechError::Vendor { .. })
    ));
}

#[tokio::test]
async fn stt_empty_text_is_no_final_and_no_error() {
    let mock = MockHttp::start(Reply::ok("application/json", br#"{"text":" "}"#.to_vec())).await;
    let mut provider = stt(&mock);
    assert!(matches!(utterance(&mut provider).await, Ok(None)));
}

/// After a failed utterance the provider is usable again: the next one succeeds.
#[tokio::test]
async fn stt_works_again_after_a_failed_request() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let n = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async move {
            let (status, body) = if n == 0 {
                ("500 Internal Server Error", "{}")
            } else {
                ("200 OK", r#"{"text":"Hello."}"#)
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
        }
    })))
    .await;
    let mut provider = stt(&mock);
    assert!(utterance(&mut provider).await.is_err());
    let got = utterance(&mut provider).await.unwrap();
    assert!(matches!(got, Some(SttTranscript::Final(ref t)) if t == "Hello."));
}
