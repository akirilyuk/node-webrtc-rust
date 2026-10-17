#![cfg(feature = "live")]
//! Local mock Cartesia server (REST + WebSocket on one port). Guards the sink contract of
//! `TtsProvider`: each PCM chunk reaches the progressive sink exactly once, and the REST
//! fallback only runs when no audio reached the sink.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, TtsSynthesis, VendorFactory,
};
use node_webrtc_rust_vendor_cartesia::CartesiaFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

const BOUND: Duration = Duration::from_secs(10);
const CHUNK_BYTES: usize = 960;

type Ws = WebSocketStream<TcpStream>;
type WsScript = Box<dyn FnOnce(Ws, String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

struct RestReply {
    status: &'static str,
    body: Vec<u8>,
}

struct Mock {
    endpoint: String,
    ws_count: Arc<AtomicUsize>,
    rest_count: Arc<AtomicUsize>,
    ws_uris: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// `ws`: script run after the generation request arrived (`None` refuses the upgrade with 403).
    async fn start(ws: Option<WsScript>, rest: RestReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let ws_count = Arc::new(AtomicUsize::new(0));
        let rest_count = Arc::new(AtomicUsize::new(0));
        let ws_uris = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(ws));
        let rest = Arc::new((rest.status, rest.body));
        {
            let ws_count = ws_count.clone();
            let rest_count = rest_count.clone();
            let ws_uris = ws_uris.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let ws_count = ws_count.clone();
                    let rest_count = rest_count.clone();
                    let ws_uris = ws_uris.clone();
                    let script = script.clone();
                    let rest = rest.clone();
                    tokio::spawn(async move {
                        let path = peek_request_path(&stream).await;
                        if path.starts_with("/tts/websocket") {
                            ws_count.fetch_add(1, Ordering::SeqCst);
                            let script = script.lock().unwrap().take();
                            match script {
                                Some(script) => handle_ws(stream, ws_uris, script).await,
                                None => refuse(stream).await,
                            }
                        } else {
                            rest_count.fetch_add(1, Ordering::SeqCst);
                            handle_rest(stream, rest.0, &rest.1).await;
                        }
                    });
                }
            });
        }
        Self {
            endpoint: format!("http://{addr}"),
            ws_count,
            rest_count,
            ws_uris,
        }
    }

    fn ws_count(&self) -> usize {
        self.ws_count.load(Ordering::SeqCst)
    }

    fn rest_count(&self) -> usize {
        self.rest_count.load(Ordering::SeqCst)
    }

    fn provider(&self) -> Box<dyn TtsProvider> {
        CartesiaFactory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Cartesia,
                model: None,
                model_path: None,
                voice: Some("test-voice".into()),
                api_key: Some("test-key".into()),
                endpoint: Some(self.endpoint.clone()),
                sentence_case: Default::default(),
            })
            .unwrap()
    }
}

async fn peek_request_path(stream: &TcpStream) -> String {
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.peek(&mut buf).await.unwrap();
        assert!(n > 0, "connection closed before a request line arrived");
        if let Some(end) = buf[..n].windows(2).position(|w| w == b"\r\n") {
            let line = String::from_utf8_lossy(&buf[..end]).to_string();
            return line.split(' ').nth(1).unwrap_or_default().to_string();
        }
    }
}

/// Reads one HTTP/1.1 request; returns the body.
async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
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
    let head = String::from_utf8_lossy(&data[..head_end]).to_ascii_lowercase();
    let content_length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < head_end + content_length {
        let n = stream.read(&mut tmp).await.unwrap();
        assert!(n > 0, "connection closed mid-body");
        data.extend_from_slice(&tmp[..n]);
    }
    data[head_end..head_end + content_length].to_vec()
}

async fn handle_rest(mut stream: TcpStream, status: &str, body: &[u8]) {
    let _ = read_http_request(&mut stream).await;
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let _ = stream.shutdown().await;
}

async fn refuse(mut stream: TcpStream) {
    let _ = read_http_request(&mut stream).await;
    stream
        .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
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
    let context_id = loop {
        match ws.next().await {
            Some(Ok(Message::Text(raw))) => {
                let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
                break value["context_id"].as_str().unwrap_or_default().to_string();
            }
            Some(Ok(_)) => continue,
            other => panic!("no generation request: {other:?}"),
        }
    };
    script(ws, context_id).await;
}

fn chunk_msg(context_id: &str, fill: u8) -> Message {
    let data = STANDARD.encode(vec![fill; CHUNK_BYTES]);
    Message::Text(
        serde_json::json!({
            "type": "chunk",
            "data": data,
            "done": false,
            "status_code": 206,
            "context_id": context_id,
        })
        .to_string()
        .into(),
    )
}

fn done_msg(context_id: &str) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "done",
            "done": true,
            "status_code": 200,
            "context_id": context_id,
        })
        .to_string()
        .into(),
    )
}

fn script<F, Fut>(f: F) -> Option<WsScript>
where
    F: FnOnce(Ws, String) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Some(Box::new(move |ws, ctx| Box::pin(f(ws, ctx))))
}

fn rest_ok(bytes: usize) -> RestReply {
    RestReply {
        status: "200 OK",
        body: vec![7u8; bytes],
    }
}

fn rest_fail() -> RestReply {
    RestReply {
        status: "500 Internal Server Error",
        body: b"rest must not be used".to_vec(),
    }
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
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            for fill in 1u8..=3 {
                ws.send(chunk_msg(&ctx, fill)).await.unwrap();
            }
            ws.send(done_msg(&ctx)).await.unwrap();
            let _ = ws.close(None).await;
        }),
        rest_fail(),
    )
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
    assert_eq!(mock.rest_count(), 0);
    let uris = mock.ws_uris.lock().unwrap().clone();
    assert_eq!(uris.len(), 1);
    assert!(uris[0].starts_with("/tts/websocket"), "{}", uris[0]);
    assert!(uris[0].contains("cartesia_version="), "{}", uris[0]);
}

#[tokio::test]
async fn ws_error_before_any_audio_falls_back_to_rest() {
    let mock = Mock::start(
        script(|mut ws, _ctx| async move {
            let _ = ws.close(None).await;
        }),
        rest_ok(CHUNK_BYTES),
    )
    .await;
    let provider = mock.provider();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    assert!(result.is_ok(), "{result:?}");
    let sunk = drain(&mut rig.rx);
    assert_eq!(sunk.len(), 1);
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2);
    assert_eq!(mock.rest_count(), 1);
}

#[tokio::test]
async fn ws_error_after_audio_does_not_fall_back() {
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            ws.send(chunk_msg(&ctx, 1)).await.unwrap();
            let _ = ws.close(None).await;
        }),
        rest_ok(CHUNK_BYTES),
    )
    .await;
    let provider = mock.provider();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    assert!(result.is_err(), "{result:?}");
    assert_eq!(drain(&mut rig.rx).len(), 1);
    assert_eq!(mock.rest_count(), 0);
}

#[tokio::test]
async fn ws_close_before_done_is_an_error_not_success() {
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            ws.send(chunk_msg(&ctx, 1)).await.unwrap();
            let _ = ws.close(None).await;
        }),
        rest_fail(),
    )
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
            assert_eq!(vendor, "cartesia");
            assert_eq!(message, "cartesia ws closed before done");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(mock.rest_count(), 0);
}

#[tokio::test]
async fn ws_server_error_message_is_a_vendor_error() {
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            let message = serde_json::json!({
                "type": "error",
                "title": "Invalid voice",
                "message": "voice not found",
                "status_code": 400,
                "done": true,
                "context_id": ctx,
            })
            .to_string();
            ws.send(Message::Text(message.into())).await.unwrap();
            let _ = ws.close(None).await;
        }),
        rest_fail(),
    )
    .await;
    let provider = mock.provider();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    // Nothing reached the sink, so the REST fallback ran (and failed with its own 500).
    assert!(result.is_err(), "{result:?}");
    assert_eq!(mock.ws_count(), 1);
    assert_eq!(mock.rest_count(), 1);
    assert!(drain(&mut rig.rx).is_empty());
}

#[tokio::test]
async fn cancel_after_the_first_chunk_stops_reading() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (report_tx, report_rx) = oneshot::channel::<bool>();
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            ws.send(chunk_msg(&ctx, 1)).await.unwrap();
            go_rx.await.unwrap();
            ws.send(chunk_msg(&ctx, 2)).await.unwrap();
            let closed = matches!(ws.next().await, Some(Ok(Message::Close(_))));
            let _ = report_tx.send(closed);
        }),
        rest_fail(),
    )
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
    assert_eq!(mock.rest_count(), 0);
}

#[tokio::test]
async fn rest_path_sends_the_utterance_to_the_sink_once() {
    let mock = Mock::start(None, rest_ok(CHUNK_BYTES * 2)).await;
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
    assert_eq!(sunk.len(), 1);
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2 * 2);
    let sink_total: usize = sunk.iter().map(|c| c.pcm.len()).sum();
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(returned_total, sink_total);
    assert_eq!(mock.rest_count(), 1);
}

/// The clock is paused only around the wait: connecting runs on the real clock, because
/// auto-advance fires a connect timeout while loopback I/O is still pending. On the
/// current-thread test runtime the client arms its first-audio timer in the same poll that
/// flushes the generation request, before the server task can read it. So when the server
/// reports "read", the timer is already armed and `advance` is enough.
#[tokio::test]
async fn ws_that_never_sends_audio_times_out_and_falls_back_to_rest() {
    let (read_tx, read_rx) = oneshot::channel::<()>();
    let mock = Mock::start(
        script(|ws, _ctx| async move {
            // Hold the socket open: a dropped socket would be a read error, not a silent server.
            let _ws = ws;
            let _ = read_tx.send(());
            std::future::pending::<()>().await;
        }),
        rest_ok(CHUNK_BYTES),
    )
    .await;
    let provider = mock.provider();
    let mut rig = sink_rig();
    let sink = rig.sink.clone();

    let call =
        tokio::spawn(async move { provider.synthesize_progressive("hello", Some(sink)).await });

    tokio::time::timeout(BOUND, read_rx)
        .await
        .expect("server never read the generation request")
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();

    let result = tokio::time::timeout(BOUND, call)
        .await
        .expect("synthesize hung after the first-audio bound")
        .unwrap();
    assert!(result.is_ok(), "{result:?}");
    let sunk = drain(&mut rig.rx);
    assert_eq!(sunk.len(), 1);
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2);
    assert_eq!(mock.ws_count(), 1);
    assert_eq!(mock.rest_count(), 1);
}

#[tokio::test]
async fn ws_that_stalls_after_audio_times_out() {
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            ws.send(chunk_msg(&ctx, 1)).await.unwrap();
            std::future::pending::<()>().await;
        }),
        rest_ok(CHUNK_BYTES),
    )
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
            assert_eq!(message, "cartesia ws stalled after 10000 ms");
        }
        other => panic!("expected a stall error, got {other:?}"),
    }
    assert_eq!(mock.rest_count(), 0);
    assert!(
        drain(&mut rig.rx).is_empty(),
        "exactly one chunk reached the sink"
    );
}
