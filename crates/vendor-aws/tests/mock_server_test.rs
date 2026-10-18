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
    chunked_head, script, write_chunk, MockHttp, Reply,
};
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
    assert!(sunk.iter().all(|c| c.pcm.len() % 12 == 0), "sample aligned");
    // Each mono sample appears three times per channel, in order.
    let stereo: Vec<u8> = sunk.iter().flat_map(|c| c.pcm.to_vec()).collect();
    let left: Vec<u8> = stereo
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|s| [s[0], s[1]])
        .collect();
    let expect: Vec<u8> = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|s| [s[0], s[1], s[0], s[1], s[0], s[1]])
        .collect();
    assert_eq!(left, expect);
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
