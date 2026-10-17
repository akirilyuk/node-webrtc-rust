#![cfg(feature = "live")]
//! Local mock ElevenLabs server (HTTP/1.1 only; the WebSocket stream-input path is not
//! selected by `tts_progressive_transport`). Guards the sink contract of `TtsProvider`: each
//! PCM chunk reaches the progressive sink exactly once, the empty-stream fallback reaches the
//! sink, a cancel never falls back, and every wait is bounded.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, TtsSynthesis, VendorFactory,
};
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

const BOUND: Duration = Duration::from_secs(10);
const CHUNK_BYTES: usize = 960;

type StreamScript = Box<dyn FnOnce(TcpStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

struct Mock {
    endpoint: String,
    stream_hits: Arc<AtomicUsize>,
    full_hits: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// `stream`: script run on the `/stream` request (after the request was read).
    /// `full_body`: PCM bytes the non-stream endpoint answers with (200).
    async fn start(stream: Option<StreamScript>, full_body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let stream_hits = Arc::new(AtomicUsize::new(0));
        let full_hits = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(stream));
        let full_body = Arc::new(full_body);
        {
            let stream_hits = stream_hits.clone();
            let full_hits = full_hits.clone();
            let paths = paths.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        break;
                    };
                    let stream_hits = stream_hits.clone();
                    let full_hits = full_hits.clone();
                    let paths = paths.clone();
                    let script = script.clone();
                    let full_body = full_body.clone();
                    tokio::spawn(async move {
                        let path = read_http_request(&mut socket).await;
                        paths.lock().unwrap().push(path.clone());
                        let route = path.split('?').next().unwrap_or_default().to_string();
                        if route.ends_with("/stream") {
                            stream_hits.fetch_add(1, Ordering::SeqCst);
                            let script = script.lock().unwrap().take();
                            match script {
                                Some(script) => script(socket).await,
                                None => {
                                    write_status(&mut socket, "500 Internal Server Error").await
                                }
                            }
                        } else {
                            full_hits.fetch_add(1, Ordering::SeqCst);
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: audio/pcm\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                full_body.len()
                            );
                            socket.write_all(head.as_bytes()).await.unwrap();
                            socket.write_all(&full_body).await.unwrap();
                            let _ = socket.shutdown().await;
                        }
                    });
                }
            });
        }
        Self {
            endpoint: format!("http://{addr}"),
            stream_hits,
            full_hits,
            paths,
        }
    }

    fn stream_hits(&self) -> usize {
        self.stream_hits.load(Ordering::SeqCst)
    }

    fn full_hits(&self) -> usize {
        self.full_hits.load(Ordering::SeqCst)
    }

    fn provider(&self, model: Option<&str>) -> Box<dyn TtsProvider> {
        ElevenLabsFactory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Elevenlabs,
                model: model.map(str::to_string),
                model_path: None,
                voice: Some("voice-1".into()),
                api_key: Some("test-key".into()),
                endpoint: Some(self.endpoint.clone()),
                sentence_case: Default::default(),
            })
            .unwrap()
    }
}

/// Reads one HTTP/1.1 request; returns the request path (with query).
async fn read_http_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut tmp).await.unwrap();
        assert!(n > 0, "connection closed mid-request");
        data.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let content_length = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:").map(str::to_string))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < head_end + content_length {
        let n = stream.read(&mut tmp).await.unwrap();
        assert!(n > 0, "connection closed mid-body");
        data.extend_from_slice(&tmp[..n]);
    }
    head.split(' ').nth(1).unwrap_or_default().to_string()
}

async fn write_status(stream: &mut TcpStream, status: &str) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn write_chunked_head(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: audio/pcm\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
        )
        .await
        .unwrap();
}

async fn write_chunk(stream: &mut TcpStream, data: &[u8]) {
    stream
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await
        .unwrap();
    stream.write_all(data).await.unwrap();
    stream.write_all(b"\r\n").await.unwrap();
}

async fn write_chunked_end(stream: &mut TcpStream) {
    stream.write_all(b"0\r\n\r\n").await.unwrap();
    let _ = stream.shutdown().await;
}

fn script<F, Fut>(f: F) -> Option<StreamScript>
where
    F: FnOnce(TcpStream) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Some(Box::new(move |stream| Box::pin(f(stream))))
}

struct SinkRig {
    sink: TtsProgressiveSink,
    rx: mpsc::UnboundedReceiver<TtsAudioChunk>,
    cancel: Arc<AtomicBool>,
}

fn sink_rig() -> SinkRig {
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    SinkRig {
        sink: TtsProgressiveSink {
            tx,
            cancel: cancel.clone(),
        },
        rx,
        cancel,
    }
}

fn drain(rx: &mut mpsc::UnboundedReceiver<TtsAudioChunk>) -> Vec<TtsAudioChunk> {
    let mut out = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        out.push(chunk);
    }
    out
}

#[tokio::test]
async fn http_stream_sends_each_chunk_to_the_sink_once() {
    let mock = Mock::start(
        script(|mut stream| async move {
            write_chunked_head(&mut stream).await;
            for fill in 1u8..=3 {
                write_chunk(&mut stream, &vec![fill; CHUNK_BYTES]).await;
            }
            write_chunked_end(&mut stream).await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let mut rig = sink_rig();

    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");

    let sunk = drain(&mut rig.rx);
    assert_eq!(
        sunk.len(),
        3,
        "each body chunk reaches the sink exactly once"
    );
    for chunk in &sunk {
        assert_eq!(chunk.pcm.len(), CHUNK_BYTES * 2);
    }
    let sink_total: usize = sunk.iter().map(|c| c.pcm.len()).sum();
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(sink_total, 5760);
    assert_eq!(returned_total, sink_total);
    assert_eq!(mock.stream_hits(), 1);
    assert_eq!(mock.full_hits(), 0);
    let paths = mock.paths.lock().unwrap().clone();
    assert!(
        paths[0].starts_with("/v1/text-to-speech/voice-1/stream?"),
        "{}",
        paths[0]
    );
}

#[tokio::test]
async fn empty_http_stream_falls_back_and_sends_the_full_body_once() {
    let mock = Mock::start(
        script(|mut stream| async move {
            write_chunked_head(&mut stream).await;
            write_chunked_end(&mut stream).await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let mut rig = sink_rig();

    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");

    let sunk = drain(&mut rig.rx);
    assert_eq!(
        sunk.len(),
        1,
        "the fallback audio reaches the sink exactly once"
    );
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2);
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(returned_total, CHUNK_BYTES * 2);
    assert_eq!(mock.stream_hits(), 1);
    assert_eq!(mock.full_hits(), 1);
}

#[tokio::test]
async fn full_body_model_sends_the_utterance_once() {
    let mock = Mock::start(None, vec![7u8; CHUNK_BYTES]).await;
    let provider = mock.provider(Some("eleven_v3"));
    let mut rig = sink_rig();

    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");

    let sunk = drain(&mut rig.rx);
    assert_eq!(sunk.len(), 1);
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2);
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(returned_total, sunk[0].pcm.len());
    assert_eq!(mock.stream_hits(), 0);
    assert_eq!(mock.full_hits(), 1);
}

#[tokio::test]
async fn cancel_after_the_first_chunk_stops_reading() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let mock = Mock::start(
        script(|mut stream| async move {
            write_chunked_head(&mut stream).await;
            write_chunk(&mut stream, &vec![1u8; CHUNK_BYTES]).await;
            go_rx.await.unwrap();
            write_chunk(&mut stream, &vec![2u8; CHUNK_BYTES]).await;
            // Keep the response open: the client must stop on its own.
            std::future::pending::<()>().await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let SinkRig {
        sink,
        mut rx,
        cancel,
    } = sink_rig();

    let synth = provider.synthesize_progressive_with_status("hello", Some(sink));
    let driver = async {
        let first = rx.recv().await.expect("first chunk");
        cancel.store(true, Ordering::SeqCst);
        go_tx.send(()).unwrap();
        first
    };
    let (result, first) = tokio::time::timeout(BOUND, async { tokio::join!(synth, driver) })
        .await
        .expect("cancel flow timed out");

    assert_eq!(first.pcm.len(), CHUNK_BYTES * 2);
    match result {
        Ok(TtsSynthesis { complete, .. }) => {
            assert!(!complete, "cancelled synthesis is incomplete")
        }
        Err(err) => panic!("cancel must not be an error: {err:?}"),
    }
    assert!(drain(&mut rx).is_empty(), "no chunk after the cancel");
    assert_eq!(mock.full_hits(), 0);
}

#[tokio::test]
async fn cancel_before_any_audio_does_not_fall_back() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let mock = Mock::start(
        script(|mut stream| async move {
            write_chunked_head(&mut stream).await;
            go_rx.await.unwrap();
            write_chunked_end(&mut stream).await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let SinkRig {
        sink,
        mut rx,
        cancel,
    } = sink_rig();

    let synth = provider.synthesize_progressive_with_status("hello", Some(sink));
    let driver = async {
        cancel.store(true, Ordering::SeqCst);
        go_tx.send(()).unwrap();
    };
    let (result, ()) = tokio::time::timeout(BOUND, async { tokio::join!(synth, driver) })
        .await
        .expect("cancel flow timed out");

    match result {
        Ok(TtsSynthesis { complete, .. }) => {
            assert!(!complete, "cancelled synthesis is incomplete")
        }
        Err(err) => panic!("cancel must not be an error: {err:?}"),
    }
    assert!(drain(&mut rx).is_empty());
    assert_eq!(
        mock.full_hits(),
        0,
        "a cancel never falls back to the full body"
    );
}

/// The clock is paused only around the wait: connecting runs on the real clock, because
/// auto-advance fires a connect timeout while loopback I/O is still pending.
///
/// The server answers nothing, not even headers. The client's first-audio timer around
/// `send()` was armed before the request was written, so it is armed whenever the server
/// reports "request read". Withholding the headers keeps the test deterministic: if the
/// server sent headers and then signalled, the client could still be waking up to read them
/// when the clock moves, and it would arm its body timer after `advance` (a 15 s real wait).
#[tokio::test]
async fn http_stream_that_never_sends_audio_times_out() {
    let (read_tx, read_rx) = oneshot::channel::<()>();
    let mock = Mock::start(
        script(|stream| async move {
            // Hold the socket open: a dropped socket would be a read error, not a silent server.
            let _stream = stream;
            let _ = read_tx.send(());
            std::future::pending::<()>().await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let rig = sink_rig();
    let sink = rig.sink.clone();

    let started = tokio::time::Instant::now();
    let call =
        tokio::spawn(async move { provider.synthesize_progressive("hello", Some(sink)).await });

    tokio::time::timeout(BOUND, read_rx)
        .await
        .expect("server never read the request")
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();

    let result = tokio::time::timeout(BOUND, call)
        .await
        .expect("synthesize hung after the first-audio bound")
        .unwrap();
    let waited = started.elapsed();
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(
                message,
                "elevenlabs http first audio timed out after 15000 ms"
            );
        }
        other => panic!("expected a first-audio timeout, got {other:?}"),
    }
    assert!(waited >= Duration::from_secs(15), "{waited:?}");
    assert!(waited < Duration::from_secs(25), "{waited:?}");
    assert_eq!(mock.full_hits(), 0);
}

#[tokio::test]
async fn http_stream_that_stalls_after_audio_times_out() {
    let mock = Mock::start(
        script(|mut stream| async move {
            write_chunked_head(&mut stream).await;
            write_chunk(&mut stream, &vec![1u8; CHUNK_BYTES]).await;
            std::future::pending::<()>().await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let mut rig = sink_rig();
    let sink = rig.sink.clone();

    let call =
        tokio::spawn(async move { provider.synthesize_progressive("hello", Some(sink)).await });

    // Receiving the first chunk means the client already armed the idle timer in the same poll.
    let first = tokio::time::timeout(BOUND, rig.rx.recv())
        .await
        .expect("first chunk timed out")
        .expect("first chunk");
    assert_eq!(first.pcm.len(), CHUNK_BYTES * 2);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::time::resume();

    let result = tokio::time::timeout(BOUND, call)
        .await
        .expect("synthesize hung after the idle bound")
        .unwrap();
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "elevenlabs http stalled after 10000 ms");
        }
        other => panic!("expected a stall error, got {other:?}"),
    }
    assert!(
        drain(&mut rig.rx).is_empty(),
        "exactly one chunk reached the sink"
    );
    assert_eq!(mock.full_hits(), 0);
}
