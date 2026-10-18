#![cfg(feature = "live")]
//! Local mock Deepgram speak server (WebSocket on one port). Guards the sink contract of
//! `TtsProvider`: each PCM chunk reaches the progressive sink exactly once, a close before
//! `Flushed` is an error, and every wait is bounded.

use std::collections::VecDeque;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
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
/// Answers a REST request after its head was read; owns the connection.
type RestScript = Arc<dyn Fn(TcpStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

enum Rest {
    /// 200 with the whole body and a Content-Length, or 500 when `None`.
    Body(Option<Vec<u8>>),
    Script(RestScript),
}

struct Mock {
    endpoint: String,
    ws_count: Arc<AtomicUsize>,
    http_count: Arc<AtomicUsize>,
    ws_uris: Arc<Mutex<Vec<String>>>,
    http_uris: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// `ws`: script run after the `Speak` and `Flush` messages arrived. REST answers 500.
    async fn start(ws: WsScript) -> Self {
        Self::start_with_rest(ws, None).await
    }

    /// One WS script per accepted WebSocket connection, in order. Connections beyond the
    /// scripts are dropped without a handshake.
    async fn start_many(ws: Vec<WsScript>) -> Self {
        Self::start_inner(ws.into(), Rest::Body(None)).await
    }

    /// REST answered by `script`; WebSocket connections get no script.
    async fn start_rest_script(script: RestScript) -> Self {
        Self::start_inner(VecDeque::new(), Rest::Script(script)).await
    }

    /// Like [`Mock::start`], and `rest_body` (when set) is the 200 body of any REST request.
    async fn start_with_rest(ws: WsScript, rest_body: Option<Vec<u8>>) -> Self {
        Self::start_inner(VecDeque::from([ws]), Rest::Body(rest_body)).await
    }

    async fn start_inner(scripts: VecDeque<WsScript>, rest: Rest) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let ws_count = Arc::new(AtomicUsize::new(0));
        let http_count = Arc::new(AtomicUsize::new(0));
        let ws_uris = Arc::new(Mutex::new(Vec::new()));
        let http_uris = Arc::new(Mutex::new(Vec::new()));
        let rest = Arc::new(rest);
        let script = Arc::new(Mutex::new(scripts));
        {
            let http_uris = http_uris.clone();
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
                    let http_uris = http_uris.clone();
                    let rest = rest.clone();
                    tokio::spawn(async move {
                        if peek_is_websocket_upgrade(&stream).await {
                            ws_count.fetch_add(1, Ordering::SeqCst);
                            let script = script.lock().unwrap().pop_front();
                            if let Some(script) = script {
                                handle_ws(stream, ws_uris, script).await;
                            }
                        } else {
                            http_count.fetch_add(1, Ordering::SeqCst);
                            handle_http(stream, http_uris, rest).await;
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
            http_uris,
        }
    }

    fn provider(&self) -> Box<dyn TtsProvider> {
        self.provider_with(None)
    }

    fn provider_with(&self, transport: Option<SpeechTransport>) -> Box<dyn TtsProvider> {
        DeepgramFactory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Deepgram,
                model: None,
                model_path: None,
                voice: None,
                api_key: Some("test-key".into()),
                endpoint: Some(self.endpoint.clone()),
                sentence_case: Default::default(),
                transport,
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

/// REST: 200 with `rest_body` when configured, else 500 so a regression shows up.
async fn handle_http(mut stream: TcpStream, uris: Arc<Mutex<Vec<String>>>, rest: Arc<Rest>) {
    let mut tmp = [0u8; 4096];
    let mut data = Vec::new();
    while !data.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            return;
        }
        data.extend_from_slice(&tmp[..n]);
    }
    let head = String::from_utf8_lossy(&data).to_string();
    if let Some(target) = head.split_whitespace().nth(1) {
        uris.lock().unwrap().push(target.to_string());
    }
    let rest_body = match rest.as_ref() {
        Rest::Script(script) => {
            script(stream).await;
            return;
        }
        Rest::Body(body) => body,
    };
    match rest_body {
        Some(body) => {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/l16\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes()).await;
            let _ = stream.write_all(body).await;
        }
        None => {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    }
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
        // The client sends the documented `Clear` before closing.
        let mut closed = false;
        while let Some(Ok(msg)) = ws.next().await {
            match msg {
                Message::Text(_) => continue,
                Message::Close(_) => {
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
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

fn ws_ok_script() -> WsScript {
    script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        ws.send(flushed_msg()).await.unwrap();
        let _ = ws.close(None).await;
    })
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
async fn explicit_websocket_transport_uses_the_ws_endpoint() {
    let mock = Mock::start_with_rest(ws_ok_script(), Some(vec![7u8; CHUNK_BYTES])).await;
    let provider = mock.provider_with(Some(SpeechTransport::Websocket));
    let (_, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(mock.ws_count.load(Ordering::SeqCst), 1);
    assert_eq!(mock.http_count.load(Ordering::SeqCst), 0);
    assert_eq!(provider.last_transport().as_deref(), Some("ws"));
}

#[tokio::test]
async fn explicit_http_transport_uses_the_rest_endpoint() {
    let mock = Mock::start_with_rest(ws_ok_script(), Some(vec![7u8; CHUNK_BYTES])).await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let (chunks, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(mock.ws_count.load(Ordering::SeqCst), 0, "no WebSocket");
    assert_eq!(mock.http_count.load(Ordering::SeqCst), 1);
    let uris = mock.http_uris.lock().unwrap().clone();
    assert!(uris[0].starts_with("/v1/speak"), "{}", uris[0]);
    // The REST body reaches the sink exactly once and equals what is returned.
    assert_eq!(sunk.len(), 1);
    assert_eq!(sunk[0].pcm.len(), CHUNK_BYTES * 2);
    let returned: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(returned, sunk[0].pcm.len());
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn auto_transport_is_the_catalog_default_websocket() {
    let mock = Mock::start_with_rest(ws_ok_script(), Some(vec![7u8; CHUNK_BYTES])).await;
    let provider = mock.provider_with(Some(SpeechTransport::Auto));
    let (_, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(mock.ws_count.load(Ordering::SeqCst), 1);
    assert_eq!(mock.http_count.load(Ordering::SeqCst), 0);
}

#[test]
fn unsupported_stt_transport_fails_at_construction() {
    let result = DeepgramFactory.create_stt(&SttConfig {
        provider: SttVendor::Deepgram,
        model: None,
        model_path: None,
        language: None,
        api_key: Some("k".into()),
        endpoint: None,
        transport: Some(SpeechTransport::Http),
    });
    match result {
        Err(SpeechError::Config(message)) => assert!(message.contains("allowed: auto, websocket")),
        Err(other) => panic!("expected a Config error, got {other:?}"),
        Ok(_) => panic!("http STT must be rejected"),
    }
}

// ---- socket reuse across utterances ----

/// Reads the `Speak` + `Flush` pair of one more utterance on an open socket.
async fn read_controls(ws: &mut Ws) {
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
}

fn sink_bytes(chunks: &[TtsAudioChunk]) -> usize {
    chunks.iter().map(|c| c.pcm.len()).sum()
}

#[tokio::test]
async fn two_utterances_reuse_one_websocket() {
    let mock = Mock::start(script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        ws.send(flushed_msg()).await.unwrap();
        read_controls(&mut ws).await;
        ws.send(audio_msg(2)).await.unwrap();
        ws.send(audio_msg(3)).await.unwrap();
        ws.send(flushed_msg()).await.unwrap();
        // Hold the socket until the client closes it.
        while ws.next().await.is_some() {}
    }))
    .await;
    let provider = mock.provider();

    let (chunks1, sunk1) = run_once(provider.as_ref()).await;
    let (chunks2, sunk2) = run_once(provider.as_ref()).await;

    assert_eq!(mock.ws_count.load(Ordering::SeqCst), 1, "one accept");
    assert_eq!(sunk1.len(), 1);
    assert_eq!(sunk2.len(), 2);
    assert_eq!(sink_bytes(&sunk1), sink_bytes(&chunks1));
    assert_eq!(sink_bytes(&sunk2), sink_bytes(&chunks2));
    assert_eq!(sunk2[0].pcm[0], 2, "second utterance carries its own audio");
}

#[tokio::test]
async fn server_close_between_utterances_reopens_and_succeeds() {
    let mock = Mock::start_many(vec![
        script(|mut ws| async move {
            ws.send(audio_msg(1)).await.unwrap();
            ws.send(flushed_msg()).await.unwrap();
            let _ = ws.close(None).await;
        }),
        ws_ok_script(),
    ])
    .await;
    let provider = mock.provider();

    let (_, sunk1) = run_once(provider.as_ref()).await;
    // Let the close frame reach the client's socket.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (chunks2, sunk2) = run_once(provider.as_ref()).await;

    assert_eq!(sunk1.len(), 1);
    assert_eq!(
        mock.ws_count.load(Ordering::SeqCst),
        2,
        "second utterance reopened"
    );
    assert_eq!(sunk2.len(), 1, "audio of the retry reaches the sink once");
    assert_eq!(sink_bytes(&sunk2), sink_bytes(&chunks2));
}

#[tokio::test]
async fn error_after_first_chunk_on_a_reused_socket_is_an_error_without_a_second_socket() {
    let mock = Mock::start_many(vec![
        script(|mut ws| async move {
            ws.send(audio_msg(1)).await.unwrap();
            ws.send(flushed_msg()).await.unwrap();
            read_controls(&mut ws).await;
            ws.send(audio_msg(2)).await.unwrap();
            let _ = ws.close(None).await;
        }),
        ws_ok_script(),
    ])
    .await;
    let provider = mock.provider();

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
            assert_eq!(message, "deepgram ws closed before Flushed");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(
        drain(&mut rig.rx).len(),
        1,
        "the chunk reached the sink once"
    );
    assert_eq!(
        mock.ws_count.load(Ordering::SeqCst),
        1,
        "no retry socket after audio"
    );
}

#[tokio::test]
async fn an_error_never_leaves_a_socket_for_reuse() {
    let mock = Mock::start_many(vec![
        script(|mut ws| async move {
            ws.send(audio_msg(1)).await.unwrap();
            ws.send(Message::Text(
                r#"{"type":"Error","description":"boom"}"#.into(),
            ))
            .await
            .unwrap();
            while ws.next().await.is_some() {}
        }),
        ws_ok_script(),
    ])
    .await;
    let provider = mock.provider();
    let rig = sink_rig();
    let first = provider
        .synthesize_progressive("a", Some(rig.sink.clone()))
        .await;
    assert!(first.is_err());
    let (_, sunk) = run_once(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(
        mock.ws_count.load(Ordering::SeqCst),
        2,
        "fresh socket after an error"
    );
}

#[tokio::test(start_paused = true)]
async fn tts_websocket_connect_that_never_handshakes_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _conn = listener.accept().await;
        std::future::pending::<()>().await;
    });
    let provider = DeepgramFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Deepgram,
            model: None,
            model_path: None,
            voice: None,
            api_key: Some("k".into()),
            endpoint: Some(endpoint),
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::Websocket),
        })
        .unwrap();
    match provider.synthesize_progressive("hello", None).await {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(
                message,
                "deepgram tts websocket connect timed out after 10 s"
            );
        }
        other => panic!("expected a connect timeout, got {other:?}"),
    }
}

// ---- REST body streams to the sink ----

fn chunked_head() -> &'static [u8] {
    b"HTTP/1.1 200 OK\r\nContent-Type: audio/l16\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
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
    assert_eq!(mock.http_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rest_body_error_after_audio_is_an_error_without_a_retry() {
    let mock = Mock::start_rest_script(Arc::new(|mut stream: TcpStream| {
        Box::pin(async move {
            // Promises more than it sends, then resets the connection.
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/l16\r\nContent-Length: {}\r\n\r\n",
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
    assert_eq!(
        mock.http_count.load(Ordering::SeqCst),
        1,
        "no retry after audio"
    );
    assert_eq!(mock.ws_count.load(Ordering::SeqCst), 0);
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
async fn rest_request_asks_for_container_none() {
    let mock = Mock::start_with_rest(ws_ok_script(), Some(vec![7u8; CHUNK_BYTES])).await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let _ = run_once(provider.as_ref()).await;
    let uris = mock.http_uris.lock().unwrap().clone();
    assert!(uris[0].contains("encoding=linear16"), "{}", uris[0]);
    assert!(uris[0].contains("container=none"), "{}", uris[0]);
}

#[tokio::test]
async fn rest_body_that_is_a_wav_container_is_an_error_and_nothing_reaches_the_sink() {
    let mut body = b"RIFF\x24\x00\x00\x00WAVEfmt ".to_vec();
    body.resize(44 + CHUNK_BYTES, 3);
    let mock = Mock::start_with_rest(ws_ok_script(), Some(body)).await;
    let provider = mock.provider_with(Some(SpeechTransport::Http));
    let mut rig = sink_rig();
    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("call timed out");
    match result {
        Err(SpeechError::Vendor { message, .. }) => assert_eq!(
            message,
            "deepgram rest returned a wav container; expected raw linear16 (container=none)"
        ),
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert!(drain(&mut rig.rx).is_empty(), "nothing reaches the sink");
}
