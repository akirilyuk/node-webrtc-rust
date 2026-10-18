#![cfg(feature = "live")]
//! Polly `AudioStream` forwarding against a local server. The client is built with fixed fake
//! credentials and the mock endpoint, so these tests never read `~/.aws`, the environment's AWS
//! variables or the network.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use node_webrtc_rust_vendor_aws::AwsTts;
use node_webrtc_rust_vendor_live_support::http_mock::{
    chunked_head, pcm_16bit_mono, script, write_chunk, MockHttp, Reply,
};
use node_webrtc_rust_vendor_live_support::tts::assert_chunks_match_source;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

const BOUND: Duration = Duration::from_secs(20);
/// 20 ms of 16 kHz mono s16le.
const CHUNK_BYTES: usize = 640;

fn tts(mock: &MockHttp) -> AwsTts {
    AwsTts::new(&TtsConfig {
        provider: TtsVendor::Aws,
        model: None,
        model_path: None,
        voice: None,
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap()
    .__with_mock_endpoint(&mock.base_url())
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
async fn audio_stream_reaches_the_sink_before_the_response_ends() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let go_rx = Arc::new(Mutex::new(Some(go_rx)));
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let go_rx = go_rx.lock().unwrap().take();
        async move {
            let _ = stream.write_all(&chunked_head("audio/pcm")).await;
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
    assert_eq!(
        first.pcm[0], 1,
        "the first output sample is the first source sample"
    );
    // 320 source samples give 319 x 3 interpolated frames now; the held last sample comes at the end.
    assert!(
        (19..=20).contains(&first.duration_ms),
        "{}",
        first.duration_ms
    );
    assert!(!call.is_finished(), "the call must still be running");
    go_tx.send(()).unwrap();

    let chunks = tokio::time::timeout(BOUND, call)
        .await
        .expect("call timed out")
        .unwrap()
        .expect("synthesize failed");
    let mut sunk = vec![first];
    while let Ok(chunk) = rx.try_recv() {
        sunk.push(chunk);
    }
    assert!(sunk.len() >= 2, "both body chunks reached the sink");
    let sink_bytes: usize = bytes_of(&sunk);
    assert_eq!(sink_bytes, bytes_of(&chunks), "each chunk exactly once");
    // 16 kHz mono becomes 48 kHz stereo: x3 samples, x2 channels.
    assert_eq!(bytes_of(&chunks), 2 * CHUNK_BYTES * 6);
    assert_eq!(mock.requests(), 1);
}

#[tokio::test]
async fn odd_chunk_sizes_stay_sample_aligned_and_in_order() {
    let pcm: Vec<u8> = (0..CHUNK_BYTES).map(|i| (i % 251) as u8).collect();
    let body = pcm.clone();
    let mock = MockHttp::start(Reply::Script(script(move |mut stream| {
        let body = body.clone();
        async move {
            let _ = stream.write_all(&chunked_head("audio/pcm")).await;
            for part in [&body[..21], &body[21..25], &body[25..301], &body[301..]] {
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
    assert_eq!(bytes_of(&sunk), pcm.len() * 6);
    assert_eq!(bytes_of(&returned), bytes_of(&sunk), "one send path");
    assert!(sunk.iter().all(|c| c.pcm.len() % 4 == 0), "frame aligned");
    // Position 0 of each source interval is exact, so every third output frame is a source
    // sample, in order; the last sample is held through the tail.
    let stereo: Vec<u8> = sunk.iter().flat_map(|c| c.pcm.to_vec()).collect();
    let left: Vec<u8> = stereo
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|s| [s[0], s[1]])
        .collect();
    let source: Vec<i16> = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    assert_eq!(left.len(), source.len() * 3 * 2);
    let anchors: Vec<i16> = left
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .step_by(3)
        .collect();
    assert_eq!(anchors, source);
}

#[tokio::test]
async fn stream_error_after_audio_is_an_error_without_a_retry() {
    let mock = MockHttp::start(Reply::Script(script(|mut stream| async move {
        // Promises more than it sends, then resets the connection.
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/pcm\r\nContent-Length: {}\r\n\r\n",
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
        "a stream that ends early is an error: {result:?}"
    );
    assert_eq!(
        drain(&mut rig.rx).len(),
        1,
        "the audio so far reached the sink once"
    );
    assert_eq!(mock.requests(), 1, "no retry after audio");
}

#[tokio::test]
async fn an_empty_stream_is_an_error() {
    let mock = MockHttp::start(Reply::ok("audio/pcm", Vec::new())).await;
    match tts(&mock).synthesize("hello").await {
        Err(SpeechError::Vendor { message, .. }) => assert_eq!(message, "Polly returned no audio"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_4xx_is_an_error() {
    let mock = MockHttp::start(Reply::Fixed {
        status: 400,
        content_type: "application/json",
        body: br#"{"message":"Invalid sample rate"}"#.to_vec(),
    })
    .await;
    assert!(matches!(
        tts(&mock).synthesize("hello").await,
        Err(SpeechError::Vendor { .. })
    ));
    assert_eq!(mock.requests(), 1);
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
async fn a_stream_that_stalls_after_audio_is_an_error() {
    let mock = MockHttp::start(Reply::Script(script(|mut stream| async move {
        let _ = stream.write_all(&chunked_head("audio/pcm")).await;
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

/// Catalog `audio.sample-rate-mismatch`: 1 s of 16 kHz mono from Polly is about 1000 ms and
/// 192000 bytes of 48 kHz stereo (before the fix it was 64000 bytes played as 48 kHz, three
/// times too fast).
#[tokio::test]
async fn one_second_of_16k_pcm_is_one_second_of_48k_stereo() {
    let mock = MockHttp::start(Reply::ok("audio/pcm", pcm_16bit_mono(16_000, 1000))).await;
    let chunks = tts(&mock).synthesize("hello").await.unwrap();
    assert_chunks_match_source("aws", &chunks, 1000);
    let bytes = bytes_of(&chunks);
    assert!((191_000..=193_000).contains(&bytes), "{bytes} bytes");
}
