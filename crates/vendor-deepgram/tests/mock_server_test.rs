#![cfg(feature = "live")]
//! Local mock Deepgram speak server (WebSocket on one port). Guards the sink contract of
//! `TtsProvider`: each PCM chunk reaches the progressive sink exactly once, a close before
//! `Flushed` is an error, and every wait is bounded.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, TtsSynthesis, VendorFactory,
};
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

const BOUND: Duration = Duration::from_secs(10);
const CHUNK_BYTES: usize = 960;

type Ws = WebSocketStream<TcpStream>;
type WsScript = Box<dyn FnOnce(Ws) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

struct Mock {
    endpoint: String,
    ws_count: Arc<AtomicUsize>,
    http_count: Arc<AtomicUsize>,
    ws_uris: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// `ws`: script run after the `Speak` and `Flush` messages arrived.
    async fn start(ws: WsScript) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let ws_count = Arc::new(AtomicUsize::new(0));
        let http_count = Arc::new(AtomicUsize::new(0));
        let ws_uris = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(Some(ws)));
        {
            let ws_count = ws_count.clone();
            let http_count = http_count.clone();
            let ws_uris = ws_uris.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let ws_count = ws_count.clone();
                    let http_count = http_count.clone();
                    let ws_uris = ws_uris.clone();
                    let script = script.clone();
                    tokio::spawn(async move {
                        if peek_is_websocket_upgrade(&stream).await {
                            ws_count.fetch_add(1, Ordering::SeqCst);
                            let script = script.lock().unwrap().take();
                            if let Some(script) = script {
                                handle_ws(stream, ws_uris, script).await;
                            }
                        } else {
                            http_count.fetch_add(1, Ordering::SeqCst);
                            handle_http_refused(stream).await;
                        }
                    });
                }
            });
        }
        Self {
            endpoint: format!("http://{addr}"),
            ws_count,
            http_count,
            ws_uris,
        }
    }

    fn provider(&self) -> Box<dyn TtsProvider> {
        DeepgramFactory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Deepgram,
                model: None,
                model_path: None,
                voice: None,
                api_key: Some("test-key".into()),
                endpoint: Some(self.endpoint.clone()),
                sentence_case: Default::default(),
            })
            .unwrap()
    }
}

/// True when the request head (peeked, not consumed) asks for a WebSocket upgrade.
async fn peek_is_websocket_upgrade(stream: &TcpStream) -> bool {
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.peek(&mut buf).await.unwrap();
        assert!(n > 0, "connection closed before a request head arrived");
        if let Some(end) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
            return head.contains("upgrade: websocket");
        }
    }
}

/// REST is never selected for the default model; answer 500 so a regression shows up.
async fn handle_http_refused(mut stream: TcpStream) {
    let mut tmp = [0u8; 4096];
    let mut data = Vec::new();
    while !data.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            return;
        }
        data.extend_from_slice(&tmp[..n]);
    }
    let _ = stream
        .write_all(
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
    let _ = stream.shutdown().await;
}

#[allow(clippy::result_large_err)] // the handshake callback signature is fixed by tungstenite
async fn handle_ws(stream: TcpStream, uris: Arc<Mutex<Vec<String>>>, script: WsScript) {
    let mut ws = tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        let req: &tokio_tungstenite::tungstenite::handshake::server::Request = req;
        uris.lock().unwrap().push(req.uri().to_string());
        Ok(resp)
    })
    .await
    .unwrap();
    // The client sends `Speak`, then `Flush`.
    let mut seen = Vec::new();
    while seen.len() < 2 {
        match ws.next().await {
            Some(Ok(Message::Text(raw))) => {
                let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
                seen.push(value["type"].as_str().unwrap_or_default().to_string());
            }
            Some(Ok(_)) => continue,
            other => panic!("client control messages missing: {other:?}"),
        }
    }
    assert_eq!(seen, ["Speak", "Flush"]);
    script(ws).await;
}

fn audio_msg(fill: u8) -> Message {
    Message::Binary(vec![fill; CHUNK_BYTES].into())
}

fn flushed_msg() -> Message {
    Message::Text(r#"{"type":"Flushed","sequence_id":0}"#.into())
}

fn script<F, Fut>(f: F) -> WsScript
where
    F: FnOnce(Ws) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Box::new(move |ws| Box::pin(f(ws)))
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
async fn ws_stream_sends_each_chunk_to_the_sink_once() {
    let mock = Mock::start(script(|mut ws| async move {
        for fill in 1u8..=3 {
            ws.send(audio_msg(fill)).await.unwrap();
        }
        ws.send(flushed_msg()).await.unwrap();
        let _ = ws.close(None).await;
    }))
    .await;
    let provider = mock.provider();
    let mut rig = sink_rig();

    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");

    let sunk = drain(&mut rig.rx);
    assert_eq!(sunk.len(), 3, "each WS chunk reaches the sink exactly once");
    for (index, chunk) in sunk.iter().enumerate() {
        assert_eq!(chunk.pcm.len(), CHUNK_BYTES * 2);
        // mono s16le -> stereo duplicates samples; the fill byte identifies the chunk
        assert_eq!(chunk.pcm[0], index as u8 + 1, "chunks arrive in order");
    }
    let sink_total: usize = sunk.iter().map(|c| c.pcm.len()).sum();
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(sink_total, 5760);
    assert_eq!(returned_total, sink_total);
    assert_eq!(mock.http_count.load(Ordering::SeqCst), 0);
    let uris = mock.ws_uris.lock().unwrap().clone();
    assert_eq!(uris.len(), 1);
    // The default model is `aura-asteria-en`, which is served on /v1/speak.
    assert!(uris[0].starts_with("/v1/speak"), "{}", uris[0]);
}

#[tokio::test]
async fn ws_close_before_flushed_is_an_error() {
    let mock = Mock::start(script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        let _ = ws.close(None).await;
    }))
    .await;
    let provider = mock.provider();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    match result {
        Err(SpeechError::Vendor { vendor, message }) => {
            assert_eq!(vendor, "deepgram");
            assert_eq!(message, "deepgram ws closed before Flushed");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(drain(&mut rig.rx).len(), 1);
}

#[tokio::test]
async fn ws_error_message_is_a_vendor_error() {
    let mock = Mock::start(script(|mut ws| async move {
        let message = serde_json::json!({
            "type": "Error",
            "description": "bad request",
            "code": "INVALID",
        })
        .to_string();
        ws.send(Message::Text(message.into())).await.unwrap();
        let _ = ws.close(None).await;
    }))
    .await;
    let provider = mock.provider();
    let rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    match result {
        Err(SpeechError::Vendor { vendor, message }) => {
            assert_eq!(vendor, "deepgram");
            assert!(message.contains("bad request"), "{message}");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_after_the_first_chunk_stops_reading() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (report_tx, report_rx) = oneshot::channel::<bool>();
    let mock = Mock::start(script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        go_rx.await.unwrap();
        ws.send(audio_msg(2)).await.unwrap();
        let closed = matches!(ws.next().await, Some(Ok(Message::Close(_))));
        let _ = report_tx.send(closed);
    }))
    .await;
    let provider = mock.provider();
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
    let server_saw_close = tokio::time::timeout(BOUND, report_rx)
        .await
        .expect("server report timed out")
        .unwrap();
    assert!(server_saw_close, "client closes the socket after a cancel");
}

/// The clock is paused only around the wait: connecting runs on the real clock, because
/// auto-advance fires a connect timeout while loopback I/O is still pending. On the
/// current-thread test runtime the client arms its first-audio timer in the same poll that
/// flushes the `Flush` message, before the server task can read it. So when the server
/// reports "read", the timer is already armed and `advance` is enough.
#[tokio::test]
async fn ws_that_never_sends_audio_times_out() {
    let (read_tx, read_rx) = oneshot::channel::<()>();
    let mock = Mock::start(script(|ws| async move {
        // Hold the socket open: a dropped socket would be a read error, not a silent server.
        let _ws = ws;
        let _ = read_tx.send(());
        std::future::pending::<()>().await;
    }))
    .await;
    let provider = mock.provider();
    let rig = sink_rig();
    let sink = rig.sink.clone();

    let started = tokio::time::Instant::now();
    let call =
        tokio::spawn(async move { provider.synthesize_progressive("hello", Some(sink)).await });

    tokio::time::timeout(BOUND, read_rx)
        .await
        .expect("server never read the client messages")
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
            assert_eq!(message, "deepgram ws first audio timed out after 15000 ms");
        }
        other => panic!("expected a first-audio timeout, got {other:?}"),
    }
    assert!(waited >= Duration::from_secs(15), "{waited:?}");
    assert!(waited < Duration::from_secs(25), "{waited:?}");
    assert_eq!(mock.ws_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ws_that_stalls_after_audio_times_out() {
    let mock = Mock::start(script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        std::future::pending::<()>().await;
    }))
    .await;
    let provider = mock.provider();
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
            assert_eq!(message, "deepgram ws stalled after 10000 ms");
        }
        other => panic!("expected a stall error, got {other:?}"),
    }
    assert!(
        drain(&mut rig.rx).is_empty(),
        "exactly one chunk reached the sink"
    );
}
