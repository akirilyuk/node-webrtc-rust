#![cfg(feature = "live")]
//! Local mock Cartesia server (REST + WebSocket on one port). Guards the sink contract of
//! `TtsProvider`: each PCM chunk reaches the progressive sink exactly once, and the REST
//! fallback only runs when no audio reached the sink.

use std::collections::VecDeque;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, TtsSynthesis, VendorFactory,
};
use node_webrtc_rust_vendor_cartesia::CartesiaFactory;
use node_webrtc_rust_vendor_live_support::tts::assert_chunks_match_source;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

const BOUND: Duration = Duration::from_secs(10);
const CHUNK_BYTES: usize = 960;

type Ws = WebSocketStream<TcpStream>;
type WsScript = Box<dyn FnOnce(Ws, String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;
/// Owns the whole WebSocket connection: reads its own generation requests (reuse tests).
type ConnScript = Box<dyn FnOnce(Ws) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;
/// Answers a REST request after its head was read; owns the connection.
type RestScript = Arc<dyn Fn(TcpStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

enum WsKind {
    /// Runs after the first generation request arrived, with its `context_id`.
    AfterRequest(WsScript),
    Raw(ConnScript),
}

struct RestReply {
    status: &'static str,
    body: Vec<u8>,
}

enum Rest {
    Reply(RestReply),
    Script(RestScript),
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
        let queue = ws.map(WsKind::AfterRequest).into_iter().collect();
        Self::start_inner(queue, Rest::Reply(rest)).await
    }

    /// One script per accepted WebSocket connection, in order. Connections beyond the scripts
    /// are refused with 403.
    async fn start_many(ws: Vec<WsKind>, rest: RestReply) -> Self {
        Self::start_inner(ws.into(), Rest::Reply(rest)).await
    }

    async fn start_rest_script(rest: RestScript) -> Self {
        Self::start_inner(VecDeque::new(), Rest::Script(rest)).await
    }

    async fn start_inner(queue: VecDeque<WsKind>, rest: Rest) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let ws_count = Arc::new(AtomicUsize::new(0));
        let rest_count = Arc::new(AtomicUsize::new(0));
        let ws_uris = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(queue));
        let rest = Arc::new(rest);
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
                    let queue = queue.clone();
                    let rest = rest.clone();
                    tokio::spawn(async move {
                        let path = peek_request_path(&stream).await;
                        if path.starts_with("/tts/websocket") {
                            ws_count.fetch_add(1, Ordering::SeqCst);
                            let next = queue.lock().unwrap().pop_front();
                            match next {
                                Some(kind) => handle_ws(stream, ws_uris, kind).await,
                                None => refuse(stream).await,
                            }
                        } else {
                            rest_count.fetch_add(1, Ordering::SeqCst);
                            match rest.as_ref() {
                                Rest::Reply(reply) => {
                                    handle_rest(stream, reply.status, &reply.body).await
                                }
                                Rest::Script(script) => {
                                    // The script owns the connection after the head was read.
                                    let mut stream = stream;
                                    let _ = read_http_request(&mut stream).await;
                                    script(stream).await;
                                }
                            }
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

    /// Pinned to `websocket`: the tests that use it are about the socket path. The catalog default
    /// is checked by `auto_transport_is_the_catalog_default_http`.
    fn provider(&self) -> Box<dyn TtsProvider> {
        self.provider_with(Some(SpeechTransport::Websocket))
    }

    fn provider_with(&self, transport: Option<SpeechTransport>) -> Box<dyn TtsProvider> {
        CartesiaFactory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Cartesia,
                model: None,
                model_path: None,
                voice: Some("test-voice".into()),
                api_key: Some("test-key".into()),
                endpoint: Some(self.endpoint.clone()),
                sentence_case: Default::default(),
                transport,
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
async fn handle_ws(stream: TcpStream, uris: Arc<Mutex<Vec<String>>>, kind: WsKind) {
    let mut ws = tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        let req: &tokio_tungstenite::tungstenite::handshake::server::Request = req;
        uris.lock().unwrap().push(req.uri().to_string());
        Ok(resp)
    })
    .await
    .unwrap();
    match kind {
        WsKind::Raw(script) => script(ws).await,
        WsKind::AfterRequest(script) => {
            let request = read_generation(&mut ws).await;
            script(
                ws,
                request["context_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
            .await;
        }
    }
}

/// Next client text message (a generation request or a cancel), as JSON.
async fn read_generation(ws: &mut Ws) -> serde_json::Value {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(raw))) => return serde_json::from_str(&raw).unwrap(),
            Some(Ok(_)) => continue,
            other => panic!("no generation request: {other:?}"),
        }
    }
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
            // The documented cancel message of this context, then the close.
            let cancel = read_generation(&mut ws).await;
            let cancelled = cancel["cancel"] == true && cancel["context_id"] == ctx.as_str();
            let closed = matches!(ws.next().await, Some(Ok(Message::Close(_))));
            let _ = report_tx.send(cancelled && closed);
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
    assert!(
        server_saw_close,
        "client sends the documented cancel for the context, then closes the socket"
    );
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
    let sink_total_bytes: usize = sunk.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(sink_total_bytes, CHUNK_BYTES * 2 * 2);
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

fn ws_ok_script() -> Option<WsScript> {
    script(|mut ws, ctx| async move {
        ws.send(chunk_msg(&ctx, 1)).await.unwrap();
        ws.send(done_msg(&ctx)).await.unwrap();
        let _ = ws.close(None).await;
    })
}

async fn run_progressive(provider: &dyn TtsProvider) -> Vec<TtsAudioChunk> {
    let mut rig = sink_rig();
    tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");
    drain(&mut rig.rx)
}

#[tokio::test]
async fn explicit_websocket_transport_uses_the_ws_endpoint() {
    let mock = Mock::start(ws_ok_script(), rest_fail()).await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));
    let sunk = run_progressive(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(mock.ws_count(), 1);
    assert_eq!(mock.rest_count(), 0);
}

#[tokio::test]
async fn explicit_http_transport_uses_rest_only() {
    // The WS script would succeed, so any WS attempt shows up in the counters.
    let mock = Mock::start(ws_ok_script(), rest_ok(CHUNK_BYTES)).await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let sunk = run_progressive(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1, "the REST body reaches the sink exactly once");
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2);
    assert_eq!(mock.ws_count(), 0, "http never opens the WebSocket");
    assert_eq!(mock.rest_count(), 1);
}

#[tokio::test]
async fn auto_transport_is_the_catalog_default_http() {
    for requested in [None, Some(SpeechTransport::Auto)] {
        let mock = Mock::start(ws_ok_script(), rest_ok(CHUNK_BYTES)).await;
        let provider = mock.provider_with(requested);
        let sunk = run_progressive(provider.as_ref()).await;
        assert_eq!(sunk.len(), 1);
        assert_eq!(mock.rest_count(), 1);
        assert_eq!(mock.ws_count(), 0);
    }
}

#[tokio::test]
async fn last_transport_reports_ws_then_http() {
    let mock = Mock::start(
        script(|mut ws, ctx| async move {
            ws.send(chunk_msg(&ctx, 1)).await.unwrap();
            ws.send(done_msg(&ctx)).await.unwrap();
            let _ = ws.close(None).await;
        }),
        rest_ok(CHUNK_BYTES),
    )
    .await;
    let ws_provider = mock.provider_with(Some(SpeechTransport::Websocket));
    assert_eq!(ws_provider.last_transport(), None);
    ws_provider.synthesize("hello").await.unwrap();
    assert_eq!(ws_provider.last_transport().as_deref(), Some("ws"));

    let http_provider = mock.provider_with(Some(SpeechTransport::Http));
    http_provider.synthesize("hello").await.unwrap();
    assert_eq!(http_provider.last_transport().as_deref(), Some("http/1.1"));
}

fn sink_bytes(chunks: &[TtsAudioChunk]) -> usize {
    chunks.iter().map(|c| c.pcm.len()).sum()
}

// ---- WebSocket reuse across utterances ----

fn raw(f: impl FnOnce(Ws) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static) -> WsKind {
    WsKind::Raw(Box::new(f))
}

/// Answers one generation request with `chunks` chunks and `done`.
async fn answer(ws: &mut Ws, chunks: u8, fill: u8) -> serde_json::Value {
    let request = read_generation(ws).await;
    let ctx = request["context_id"].as_str().unwrap().to_string();
    for n in 0..chunks {
        ws.send(chunk_msg(&ctx, fill + n)).await.unwrap();
    }
    ws.send(done_msg(&ctx)).await.unwrap();
    request
}

async fn run_once(provider: &dyn TtsProvider) -> (Vec<TtsAudioChunk>, Vec<TtsAudioChunk>) {
    let mut rig = sink_rig();
    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");
    (chunks, drain(&mut rig.rx))
}

#[tokio::test]
async fn two_utterances_reuse_one_websocket_with_distinct_contexts() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let seen = contexts.clone();
    let mock = Mock::start_many(
        vec![raw(move |mut ws| {
            Box::pin(async move {
                let first = answer(&mut ws, 1, 1).await;
                let second = answer(&mut ws, 2, 2).await;
                seen.lock().unwrap().push(first["context_id"].clone());
                seen.lock().unwrap().push(second["context_id"].clone());
                assert_eq!(second["continue"], false);
                // Hold the socket until the client closes it.
                while ws.next().await.is_some() {}
            })
        })],
        rest_fail(),
    )
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));

    let (chunks1, sunk1) = run_once(provider.as_ref()).await;
    let (chunks2, sunk2) = run_once(provider.as_ref()).await;

    assert_eq!(mock.ws_count(), 1, "one accept for two utterances");
    assert_eq!(mock.rest_count(), 0);
    assert_eq!(sunk1.len(), 1);
    assert_eq!(sunk2.len(), 2);
    assert_eq!(sink_bytes(&sunk1), sink_bytes(&chunks1));
    assert_eq!(sink_bytes(&sunk2), sink_bytes(&chunks2));
    assert_eq!(sunk2[0].pcm[0], 2, "second utterance carries its own audio");
    let contexts = contexts.lock().unwrap().clone();
    assert_eq!(contexts.len(), 2);
    assert_ne!(contexts[0], contexts[1], "a fresh context_id per utterance");
}

#[tokio::test]
async fn server_close_between_utterances_reopens_and_succeeds() {
    let mock = Mock::start_many(
        vec![
            raw(|mut ws| {
                Box::pin(async move {
                    answer(&mut ws, 1, 1).await;
                    let _ = ws.close(None).await;
                })
            }),
            raw(|mut ws| {
                Box::pin(async move {
                    answer(&mut ws, 1, 9).await;
                    while ws.next().await.is_some() {}
                })
            }),
        ],
        rest_fail(),
    )
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));

    let (_, sunk1) = run_once(provider.as_ref()).await;
    // Let the close frame reach the client's socket.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (chunks2, sunk2) = run_once(provider.as_ref()).await;

    assert_eq!(sunk1.len(), 1);
    assert_eq!(mock.ws_count(), 2, "second utterance reopened");
    assert_eq!(sunk2.len(), 1, "audio of the retry reaches the sink once");
    assert_eq!(sink_bytes(&sunk2), sink_bytes(&chunks2));
    assert_eq!(sunk2[0].pcm[0], 9);
    assert_eq!(mock.rest_count(), 0);
}

#[tokio::test]
async fn error_after_first_chunk_on_a_reused_socket_is_an_error_without_a_second_socket() {
    let mock = Mock::start_many(
        vec![
            raw(|mut ws| {
                Box::pin(async move {
                    answer(&mut ws, 1, 1).await;
                    let request = read_generation(&mut ws).await;
                    let ctx = request["context_id"].as_str().unwrap().to_string();
                    ws.send(chunk_msg(&ctx, 2)).await.unwrap();
                    let _ = ws.close(None).await;
                })
            }),
            raw(|mut ws| {
                Box::pin(async move {
                    answer(&mut ws, 1, 3).await;
                })
            }),
        ],
        rest_ok(CHUNK_BYTES),
    )
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));

    let (_, sunk1) = run_once(provider.as_ref()).await;
    assert_eq!(sunk1.len(), 1);

    let mut rig = sink_rig();
    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("again", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "cartesia ws closed before done");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(
        drain(&mut rig.rx).len(),
        1,
        "the chunk reached the sink once"
    );
    assert_eq!(mock.ws_count(), 1, "no retry socket after audio");
    assert_eq!(mock.rest_count(), 0, "no REST fallback after audio");
}

#[tokio::test]
async fn an_error_never_leaves_a_socket_for_reuse() {
    let mock = Mock::start_many(
        vec![
            raw(|mut ws| {
                Box::pin(async move {
                    let request = read_generation(&mut ws).await;
                    let ctx = request["context_id"].as_str().unwrap().to_string();
                    ws.send(chunk_msg(&ctx, 1)).await.unwrap();
                    let message = serde_json::json!({
                        "type": "error", "message": "boom", "done": true, "context_id": ctx,
                    });
                    ws.send(Message::Text(message.to_string().into()))
                        .await
                        .unwrap();
                    while ws.next().await.is_some() {}
                })
            }),
            raw(|mut ws| {
                Box::pin(async move {
                    answer(&mut ws, 1, 5).await;
                    while ws.next().await.is_some() {}
                })
            }),
        ],
        rest_fail(),
    )
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));
    let rig = sink_rig();
    let first = provider
        .synthesize_progressive("a", Some(rig.sink.clone()))
        .await;
    assert!(first.is_err());
    let (_, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(mock.ws_count(), 2, "fresh socket after an error");
}

#[tokio::test]
async fn messages_of_another_context_are_ignored() {
    let mock = Mock::start_many(
        vec![raw(|mut ws| {
            Box::pin(async move {
                let request = read_generation(&mut ws).await;
                let ctx = request["context_id"].as_str().unwrap().to_string();
                // Leftovers of an earlier context must not reach the sink or end this one.
                ws.send(chunk_msg("ctx-old", 7)).await.unwrap();
                ws.send(done_msg("ctx-old")).await.unwrap();
                ws.send(chunk_msg(&ctx, 1)).await.unwrap();
                ws.send(done_msg(&ctx)).await.unwrap();
                while ws.next().await.is_some() {}
            })
        })],
        rest_fail(),
    )
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));
    let (_, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(sunk[0].pcm[0], 1);
}

#[tokio::test(start_paused = true)]
async fn tts_websocket_connect_that_never_handshakes_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _conn = listener.accept().await;
        std::future::pending::<()>().await;
    });
    let provider = CartesiaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Cartesia,
            model: None,
            model_path: None,
            voice: Some("v".into()),
            api_key: Some("k".into()),
            endpoint: Some(endpoint),
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::Websocket),
        })
        .unwrap();
    // The REST fallback after the failed connect cannot connect either; the call still ends.
    let result = provider.synthesize_progressive("hello", None).await;
    assert!(result.is_err(), "{result:?}");
}

// ---- REST body streams to the sink ----

fn chunked_head() -> &'static [u8] {
    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
}

async fn write_chunk(stream: &mut TcpStream, data: &[u8]) {
    stream
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await
        .unwrap();
    stream.write_all(data).await.unwrap();
    stream.write_all(b"\r\n").await.unwrap();
    stream.flush().await.unwrap();
}

#[tokio::test]
async fn rest_body_reaches_the_sink_before_the_response_ends() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let go_rx = Arc::new(Mutex::new(Some(go_rx)));
    let mock = Mock::start_rest_script(Arc::new(move |mut stream: TcpStream| {
        let go_rx = go_rx.lock().unwrap().take();
        Box::pin(async move {
            stream.write_all(chunked_head()).await.unwrap();
            write_chunk(&mut stream, &[1u8; CHUNK_BYTES]).await;
            // The second chunk is released only after the client showed the first to the sink.
            if let Some(go_rx) = go_rx {
                let _ = go_rx.await;
            }
            write_chunk(&mut stream, &[2u8; CHUNK_BYTES]).await;
            stream.write_all(b"0\r\n\r\n").await.unwrap();
            let _ = stream.shutdown().await;
        })
    }))
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let SinkRig { sink, mut rx, .. } = sink_rig();

    let call =
        tokio::spawn(async move { provider.synthesize_progressive("hello", Some(sink)).await });
    let first = tokio::time::timeout(BOUND, rx.recv())
        .await
        .expect("no chunk before the body ended")
        .expect("first chunk");
    assert_eq!(first.pcm[0], 1);
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
    assert_eq!(sink_bytes(&chunks), 2 * CHUNK_BYTES * 2);
    assert_eq!(mock.rest_count(), 1);
    assert_eq!(mock.ws_count(), 0);
}

#[tokio::test]
async fn rest_body_error_after_audio_is_an_error_without_a_retry() {
    let mock = Mock::start_rest_script(Arc::new(|mut stream: TcpStream| {
        Box::pin(async move {
            // Promises more than it sends, then resets the connection.
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                CHUNK_BYTES * 10
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&[1u8; CHUNK_BYTES]).await.unwrap();
            stream.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(stream);
        })
    }))
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let mut rig = sink_rig();

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
        "the audio sent so far reached the sink once"
    );
    assert_eq!(mock.rest_count(), 1, "no retry after audio");
    assert_eq!(mock.ws_count(), 0);
}

#[tokio::test]
async fn rest_odd_chunk_sizes_stay_sample_aligned() {
    let mock = Mock::start_rest_script(Arc::new(|mut stream: TcpStream| {
        Box::pin(async move {
            stream.write_all(chunked_head()).await.unwrap();
            let body = vec![9u8; 1920];
            for part in [&body[..21], &body[21..25], &body[25..1001], &body[1001..]] {
                write_chunk(&mut stream, part).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            stream.write_all(b"0\r\n\r\n").await.unwrap();
            let _ = stream.shutdown().await;
        })
    }))
    .await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let (chunks, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(sink_bytes(&sunk), 1920 * 2);
    assert_eq!(sink_bytes(&chunks), sink_bytes(&sunk));
    assert!(sunk.iter().all(|c| c.pcm.len() % 4 == 0), "sample aligned");
}

#[tokio::test]
async fn rest_empty_body_is_an_error() {
    let mock = Mock::start(None, rest_ok(0)).await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let result = provider.synthesize_progressive("hello", None).await;
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(message, "Cartesia bytes returned no audio");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn missing_voice_is_a_config_error_before_any_request() {
    let mock = Mock::start(ws_ok_script(), rest_ok(CHUNK_BYTES)).await;
    let provider = CartesiaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Cartesia,
            model: None,
            model_path: None,
            voice: None,
            api_key: Some("k".into()),
            endpoint: Some(mock.endpoint.clone()),
            sentence_case: Default::default(),
            transport: None,
        })
        .unwrap();
    // CARTESIA_VOICE_ID may be set in the environment; only assert when it is not.
    if std::env::var("CARTESIA_VOICE_ID").is_err() {
        match provider.synthesize_progressive("hello", None).await {
            Err(SpeechError::Config(message)) => assert!(message.contains("CARTESIA_VOICE_ID")),
            other => panic!("{other:?}"),
        }
        assert_eq!(mock.ws_count() + mock.rest_count(), 0);
    }
}

/// Catalog `audio.sample-rate-mismatch`: 1 s of 48 kHz mono from `/tts/bytes` is 1000 ms and
/// 192000 bytes of 48 kHz stereo.
#[tokio::test]
async fn one_second_of_source_audio_is_one_second_of_48k_stereo() {
    let mock = Mock::start(None, rest_ok(96_000)).await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let (chunks, sunk) = run_once(provider.as_ref()).await;
    assert_chunks_match_source("cartesia_http", &chunks, 1000);
    assert_chunks_match_source("cartesia_http_sink", &sunk, 1000);
    assert_eq!(sink_bytes(&chunks), 192_000);
}
