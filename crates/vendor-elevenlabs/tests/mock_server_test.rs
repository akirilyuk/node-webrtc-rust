#![cfg(feature = "live")]
//! Local mock ElevenLabs server (HTTP/1.1 and WebSocket on one port). Guards the sink contract
//! of `TtsProvider`: each PCM chunk reaches the progressive sink exactly once, the empty-stream
//! fallback reaches the sink, a cancel never falls back, and every wait is bounded. The
//! WebSocket stream-input path is selected by the `websocket` customer transport; most of its
//! tests call `ElevenLabsTts::synthesize_websocket_stream` directly and guard that a close or
//! socket error before `isFinal` is an error.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, TtsSynthesis, VendorFactory,
};
use node_webrtc_rust_vendor_elevenlabs::{ElevenLabsFactory, ElevenLabsTts};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

const BOUND: Duration = Duration::from_secs(10);
const CHUNK_BYTES: usize = 960;

type Ws = WebSocketStream<TcpStream>;
type WsScript = Box<dyn FnOnce(Ws) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;
type StreamScript = Box<dyn FnOnce(TcpStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

struct Mock {
    endpoint: String,
    stream_hits: Arc<AtomicUsize>,
    full_hits: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
    ws_hits: Arc<AtomicUsize>,
    ws_uris: Arc<Mutex<Vec<String>>>,
    /// Text messages the client sent on the WebSocket, in order.
    ws_client_messages: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Mock {
    /// `stream`: script run on the `/stream` request (after the request was read).
    /// `full_body`: PCM bytes the non-stream endpoint answers with (200).
    async fn start(stream: Option<StreamScript>, full_body: Vec<u8>) -> Self {
        Self::start_with(stream, None, full_body).await
    }

    /// WebSocket mock: `ws` runs after the three client messages (init, text + flush, end of
    /// stream) arrived.
    async fn start_ws(ws: WsScript) -> Self {
        Self::start_with(None, Some(ws), vec![7u8; CHUNK_BYTES]).await
    }

    async fn start_with(
        stream: Option<StreamScript>,
        ws: Option<WsScript>,
        full_body: Vec<u8>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let stream_hits = Arc::new(AtomicUsize::new(0));
        let full_hits = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(stream));
        let ws_script = Arc::new(Mutex::new(ws));
        let ws_hits = Arc::new(AtomicUsize::new(0));
        let ws_uris = Arc::new(Mutex::new(Vec::new()));
        let ws_client_messages = Arc::new(Mutex::new(Vec::new()));
        let full_body = Arc::new(full_body);
        {
            let stream_hits = stream_hits.clone();
            let full_hits = full_hits.clone();
            let paths = paths.clone();
            let ws_hits = ws_hits.clone();
            let ws_uris = ws_uris.clone();
            let ws_client_messages = ws_client_messages.clone();
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
                    let ws_script = ws_script.clone();
                    let ws_hits = ws_hits.clone();
                    let ws_uris = ws_uris.clone();
                    let ws_client_messages = ws_client_messages.clone();
                    tokio::spawn(async move {
                        if peek_is_websocket_upgrade(&socket).await {
                            ws_hits.fetch_add(1, Ordering::SeqCst);
                            let ws_script = ws_script.lock().unwrap().take();
                            if let Some(ws_script) = ws_script {
                                handle_ws(socket, ws_uris, ws_client_messages, ws_script).await;
                            }
                            return;
                        }
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
            ws_hits,
            ws_uris,
            ws_client_messages,
        }
    }

    fn stream_hits(&self) -> usize {
        self.stream_hits.load(Ordering::SeqCst)
    }

    fn full_hits(&self) -> usize {
        self.full_hits.load(Ordering::SeqCst)
    }

    fn ws_hits(&self) -> usize {
        self.ws_hits.load(Ordering::SeqCst)
    }

    /// The concrete provider, to call `synthesize_websocket_stream` directly.
    fn elevenlabs(&self) -> ElevenLabsTts {
        ElevenLabsTts::new(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
            model: None,
            model_path: None,
            voice: Some("voice-1".into()),
            api_key: Some("test-key".into()),
            endpoint: Some(self.endpoint.clone()),
            sentence_case: Default::default(),
            transport: None,
        })
        .unwrap()
    }

    fn provider(&self, model: Option<&str>) -> Box<dyn TtsProvider> {
        self.provider_with(model, None)
    }

    fn provider_with(
        &self,
        model: Option<&str>,
        transport: Option<SpeechTransport>,
    ) -> Box<dyn TtsProvider> {
        ElevenLabsFactory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Elevenlabs,
                model: model.map(str::to_string),
                model_path: None,
                voice: Some("voice-1".into()),
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

#[allow(clippy::result_large_err)] // the handshake callback signature is fixed by tungstenite
async fn handle_ws(
    stream: TcpStream,
    uris: Arc<Mutex<Vec<String>>>,
    client_messages: Arc<Mutex<Vec<serde_json::Value>>>,
    script: WsScript,
) {
    let mut ws = tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        let req: &tokio_tungstenite::tungstenite::handshake::server::Request = req;
        uris.lock().unwrap().push(req.uri().to_string());
        Ok(resp)
    })
    .await
    .unwrap();
    // The client sends the init message, the text with `flush`, then the end of stream.
    while client_messages.lock().unwrap().len() < 3 {
        match ws.next().await {
            Some(Ok(Message::Text(raw))) => {
                let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
                client_messages.lock().unwrap().push(value);
            }
            Some(Ok(_)) => continue,
            other => panic!("client control messages missing: {other:?}"),
        }
    }
    script(ws).await;
}

fn audio_msg(fill: u8) -> Message {
    let audio = STANDARD.encode(vec![fill; CHUNK_BYTES]);
    Message::Text(
        serde_json::json!({"audio": audio, "normalizedAlignment": null, "alignment": null})
            .to_string()
            .into(),
    )
}

fn final_msg() -> Message {
    Message::Text(r#"{"isFinal":true}"#.into())
}

fn ws_script<F, Fut>(f: F) -> WsScript
where
    F: FnOnce(Ws) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Box::new(move |ws| Box::pin(f(ws)))
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

/// The 15 s first-audio deadline is armed before the request is written, so it holds whether
/// the client is still in `send()` or already waiting for the first body chunk when the clock
/// advances.
#[tokio::test]
async fn http_stream_headers_without_audio_times_out() {
    let (head_tx, head_rx) = oneshot::channel::<()>();
    let mock = Mock::start(
        script(|mut stream| async move {
            write_chunked_head(&mut stream).await;
            let _ = head_tx.send(());
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

    tokio::time::timeout(BOUND, head_rx)
        .await
        .expect("server never answered")
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();

    let result = tokio::time::timeout(BOUND, call)
        .await
        .expect("synthesize hung after the first-audio bound")
        .unwrap();
    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(
                message,
                "elevenlabs http first audio timed out after 15000 ms"
            );
        }
        other => panic!("expected a first-audio timeout, got {other:?}"),
    }
    assert_eq!(mock.full_hits(), 0);
    assert!(drain(&mut rig.rx).is_empty());
}

#[tokio::test]
async fn http_error_status_returns_the_body_as_the_error() {
    let mock = Mock::start(
        script(|mut stream| async move {
            stream
                .write_all(
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\nConnection: close\r\n\r\nboom",
                )
                .await
                .unwrap();
            let _ = stream.shutdown().await;
        }),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider(None);
    let rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_progressive("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert!(message.contains("boom"), "{message}");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
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

#[tokio::test]
async fn ws_stream_sends_each_chunk_to_the_sink_once() {
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        for fill in 1u8..=3 {
            ws.send(audio_msg(fill)).await.unwrap();
        }
        ws.send(final_msg()).await.unwrap();
        let _ = ws.close(None).await;
    }))
    .await;
    let provider = mock.elevenlabs();
    let mut rig = sink_rig();

    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_websocket_stream("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("synthesize failed");

    let sunk = drain(&mut rig.rx);
    assert_eq!(sunk.len(), 3, "each audio message reaches the sink once");
    for chunk in &sunk {
        assert_eq!(chunk.pcm.len(), CHUNK_BYTES * 2);
    }
    let sink_total: usize = sunk.iter().map(|c| c.pcm.len()).sum();
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(sink_total, 5760);
    assert_eq!(returned_total, sink_total);
    assert_eq!(mock.ws_hits(), 1);
    assert_eq!(mock.stream_hits(), 0);
    assert_eq!(mock.full_hits(), 0);
    let uris = mock.ws_uris.lock().unwrap().clone();
    assert!(
        uris[0].starts_with("/v1/text-to-speech/voice-1/stream-input?model_id="),
        "{}",
        uris[0]
    );
    let sent = mock.ws_client_messages.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![
            serde_json::json!({
                "text": " ",
                "voice_settings": {"stability": 0.5, "similarity_boost": 0.8, "use_speaker_boost": false},
            }),
            serde_json::json!({"text": "hello", "flush": true}),
            serde_json::json!({"text": ""}),
        ]
    );
}

#[tokio::test]
async fn ws_close_before_is_final_is_an_error() {
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        let _ = ws.close(None).await;
    }))
    .await;
    let provider = mock.elevenlabs();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_websocket_stream("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    match result {
        Err(SpeechError::Vendor { vendor, message }) => {
            assert_eq!(vendor, "elevenlabs");
            assert_eq!(message, "elevenlabs ws closed before isFinal");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(drain(&mut rig.rx).len(), 1);
}

#[tokio::test]
async fn ws_close_reason_is_in_the_error() {
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        let _ = ws
            .close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "quota exceeded".into(),
            }))
            .await;
    }))
    .await;
    let provider = mock.elevenlabs();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_websocket_stream("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    match result {
        Err(SpeechError::Vendor { message, .. }) => {
            assert_eq!(
                message,
                "elevenlabs ws closed before isFinal: quota exceeded"
            );
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(drain(&mut rig.rx).len(), 1);
}

#[tokio::test]
async fn ws_socket_drop_before_is_final_is_an_error() {
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        // Drop the TCP stream without a Close frame.
        drop(ws);
    }))
    .await;
    let provider = mock.elevenlabs();
    let mut rig = sink_rig();

    let result = tokio::time::timeout(
        BOUND,
        provider.synthesize_websocket_stream("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out");

    match result {
        Err(SpeechError::Vendor { vendor, message }) => {
            assert_eq!(vendor, "elevenlabs");
            assert!(message.starts_with("elevenlabs ws "), "{message}");
        }
        other => panic!("expected a vendor error, got {other:?}"),
    }
    assert_eq!(drain(&mut rig.rx).len(), 1);
}

#[tokio::test]
async fn ws_ping_does_not_end_the_stream() {
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        ws.send(Message::Ping(vec![1u8, 2, 3].into()))
            .await
            .unwrap();
        ws.send(audio_msg(2)).await.unwrap();
        ws.send(final_msg()).await.unwrap();
        let _ = ws.close(None).await;
    }))
    .await;
    let provider = mock.elevenlabs();
    let mut rig = sink_rig();

    let chunks = tokio::time::timeout(
        BOUND,
        provider.synthesize_websocket_stream("hello", Some(rig.sink.clone())),
    )
    .await
    .expect("synthesize timed out")
    .expect("a ping must not end the stream");

    assert_eq!(drain(&mut rig.rx).len(), 2);
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(returned_total, CHUNK_BYTES * 2 * 2);
}

#[tokio::test]
async fn ws_cancel_after_the_first_chunk_stops_reading() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (report_tx, report_rx) = oneshot::channel::<bool>();
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        go_rx.await.unwrap();
        ws.send(audio_msg(2)).await.unwrap();
        let closed = matches!(ws.next().await, Some(Ok(Message::Close(_))));
        let _ = report_tx.send(closed);
    }))
    .await;
    let provider = mock.elevenlabs();
    let SinkRig {
        sink,
        mut rx,
        cancel,
    } = sink_rig();

    let synth = provider.synthesize_websocket_stream("hello", Some(sink));
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
    result.expect("cancel must not be an error");
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
/// flushes the last client message, before the server task can read it. So when the server
/// reports "read", the timer is already armed and `advance` is enough.
#[tokio::test]
async fn ws_that_never_sends_audio_times_out() {
    let (read_tx, read_rx) = oneshot::channel::<()>();
    let mock = Mock::start_ws(ws_script(|ws| async move {
        // Hold the socket open: a dropped socket would be a read error, not a silent server.
        let _ws = ws;
        let _ = read_tx.send(());
        std::future::pending::<()>().await;
    }))
    .await;
    let provider = mock.elevenlabs();
    let rig = sink_rig();
    let sink = rig.sink.clone();

    let started = tokio::time::Instant::now();
    let call = tokio::spawn(async move {
        provider
            .synthesize_websocket_stream("hello", Some(sink))
            .await
    });

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
            assert_eq!(
                message,
                "elevenlabs ws first audio timed out after 15000 ms"
            );
        }
        other => panic!("expected a first-audio timeout, got {other:?}"),
    }
    assert!(waited >= Duration::from_secs(15), "{waited:?}");
    assert!(waited < Duration::from_secs(25), "{waited:?}");
    assert_eq!(mock.ws_hits(), 1);
}

#[tokio::test]
async fn ws_that_stalls_after_audio_times_out() {
    let mock = Mock::start_ws(ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        // Hold the socket open: a dropped socket would be a read error, not a stall.
        let _ws = ws;
        std::future::pending::<()>().await;
    }))
    .await;
    let provider = mock.elevenlabs();
    let mut rig = sink_rig();
    let sink = rig.sink.clone();

    let call = tokio::spawn(async move {
        provider
            .synthesize_websocket_stream("hello", Some(sink))
            .await
    });

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
            assert_eq!(message, "elevenlabs ws stalled after 10000 ms");
        }
        other => panic!("expected a stall error, got {other:?}"),
    }
    assert!(
        drain(&mut rig.rx).is_empty(),
        "exactly one chunk reached the sink"
    );
}

fn http_stream_script() -> Option<StreamScript> {
    script(|mut stream| async move {
        write_chunked_head(&mut stream).await;
        write_chunk(&mut stream, &vec![1u8; CHUNK_BYTES]).await;
        write_chunked_end(&mut stream).await;
    })
}

fn ws_ok_script() -> WsScript {
    ws_script(|mut ws| async move {
        ws.send(audio_msg(1)).await.unwrap();
        ws.send(final_msg()).await.unwrap();
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
async fn explicit_websocket_transport_goes_to_the_ws_endpoint() {
    let mock = Mock::start_with(
        http_stream_script(),
        Some(ws_ok_script()),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider_with(None, Some(SpeechTransport::Websocket));
    let sunk = run_progressive(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1, "the WS audio reaches the sink once");
    assert_eq!(mock.ws_hits(), 1);
    assert_eq!(mock.stream_hits(), 0);
    assert_eq!(mock.full_hits(), 0);
    assert_eq!(provider.last_transport().as_deref(), Some("ws"));
}

#[tokio::test]
async fn explicit_http_transport_goes_to_the_stream_endpoint() {
    let mock = Mock::start_with(
        http_stream_script(),
        Some(ws_ok_script()),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider_with(None, Some(SpeechTransport::Http));
    let sunk = run_progressive(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(mock.stream_hits(), 1);
    assert_eq!(mock.ws_hits(), 0);
    assert_eq!(provider.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn auto_transport_is_the_catalog_default_http() {
    let mock = Mock::start_with(
        http_stream_script(),
        Some(ws_ok_script()),
        vec![7u8; CHUNK_BYTES],
    )
    .await;
    let provider = mock.provider_with(None, Some(SpeechTransport::Auto));
    let sunk = run_progressive(provider.as_ref()).await;
    assert_eq!(sunk.len(), 1);
    assert_eq!(mock.stream_hits(), 1);
    assert_eq!(mock.ws_hits(), 0);
}

#[test]
fn websocket_transport_is_rejected_for_eleven_v3_at_construction() {
    let result = ElevenLabsFactory.create_tts(&TtsConfig {
        provider: TtsVendor::Elevenlabs,
        model: Some("eleven_v3".into()),
        model_path: None,
        voice: None,
        api_key: Some("k".into()),
        endpoint: None,
        sentence_case: Default::default(),
        transport: Some(SpeechTransport::Websocket),
    });
    match result {
        Err(SpeechError::Config(message)) => assert!(message.contains("eleven_v3"), "{message}"),
        Err(other) => panic!("expected a Config error, got {other:?}"),
        Ok(_) => panic!("websocket + eleven_v3 must be rejected"),
    }
}

#[test]
fn unsupported_stt_transport_fails_at_construction() {
    let result = ElevenLabsFactory.create_stt(&SttConfig {
        provider: SttVendor::Elevenlabs,
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

/// A server that accepts the TCP connection and never completes the WebSocket upgrade: the
/// connect fails at the named 10 s bound (`infra.unbounded-external-wait`).
#[tokio::test(start_paused = true)]
async fn tts_websocket_connect_that_never_handshakes_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _conn = listener.accept().await;
        std::future::pending::<()>().await;
    });
    let provider = ElevenLabsFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
            model: None,
            model_path: None,
            voice: Some("voice-1".into()),
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
                "elevenlabs tts websocket connect timed out after 10 s"
            );
        }
        other => panic!("expected a connect timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn http_stream_with_odd_chunk_sizes_stays_sample_aligned() {
    // A known ramp of 16-bit samples, split into 3- and 5-byte chunks (every split lands
    // mid-sample at least once).
    let samples: Vec<u8> = (0u16..32).flat_map(|n| (n * 100).to_le_bytes()).collect();
    let wire = samples.clone();
    let mock = Mock::start(
        script(move |mut stream| async move {
            write_chunked_head(&mut stream).await;
            let mut rest = wire.as_slice();
            let mut size = 3;
            while !rest.is_empty() {
                let take = size.min(rest.len());
                write_chunk(&mut stream, &rest[..take]).await;
                rest = &rest[take..];
                size = if size == 3 { 5 } else { 3 };
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
    // Stereo output duplicates every mono sample: fold it back to compare with the ramp.
    let mono_back: Vec<u8> = sunk
        .iter()
        .flat_map(|c| c.pcm.chunks(4).flat_map(|frame| frame[..2].to_vec()))
        .collect();
    assert_eq!(mono_back, samples, "sink audio equals the ramp");
    assert!(sunk.iter().all(|c| c.pcm.len() % 4 == 0));
    let sink_total: usize = sunk.iter().map(|c| c.pcm.len()).sum();
    let returned_total: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    assert_eq!(sink_total, returned_total);
}
