//! Mock-server tests for the Fish Audio TTS client. Run with `--features live`.
//!
//! One local `TcpListener` serves both transports: a request with `Upgrade: websocket` goes to
//! the WS handler (`accept_hdr_async`, headers captured), anything else goes to a minimal
//! HTTP/1.1 responder. Ordering uses channels and barriers, never sleeps.
#![cfg(feature = "live")]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_vendor_fishaudio::FishAudioFactory;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_hdr_async, WebSocketStream};

type BoxFut = Pin<Box<dyn Future<Output = ()> + Send>>;

struct WsConn {
    ws: WebSocketStream<TcpStream>,
    headers: HashMap<String, String>,
}

struct HttpReq {
    headers: HashMap<String, String>,
    body: Vec<u8>,
    stream: TcpStream,
}

type WsHandler = Box<dyn FnOnce(WsConn) -> BoxFut + Send>;
type HttpHandler = Box<dyn FnOnce(HttpReq) -> BoxFut + Send>;

#[derive(Default)]
struct Shared {
    ws: Mutex<Option<WsHandler>>,
    http: Mutex<Option<HttpHandler>>,
    connections: AtomicUsize,
    ws_hits: AtomicUsize,
    http_hits: AtomicUsize,
}

struct Mock {
    port: u16,
    shared: Arc<Shared>,
    accept: tokio::task::JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl Mock {
    async fn start(ws: Option<WsHandler>, http: Option<HttpHandler>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::new(Shared {
            ws: Mutex::new(ws),
            http: Mutex::new(http),
            ..Shared::default()
        });
        let accept_shared = shared.clone();
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                accept_shared.connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(route(stream, accept_shared.clone()));
            }
        });
        Mock {
            port,
            shared,
            accept,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    fn ws_hits(&self) -> usize {
        self.shared.ws_hits.load(Ordering::SeqCst)
    }

    fn http_hits(&self) -> usize {
        self.shared.http_hits.load(Ordering::SeqCst)
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_headers(head: &str) -> HashMap<String, String> {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect()
}

/// Reads the request head (and the body, per `content-length`) off the socket.
async fn read_request(stream: &mut TcpStream) -> (HashMap<String, String>, Vec<u8>) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "client closed before sending a full request head");
        buf.extend_from_slice(&chunk[..n]);
    };
    let headers = parse_headers(&String::from_utf8_lossy(&buf[..head_end]));
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "client closed before sending the full body");
        body.extend_from_slice(&chunk[..n]);
    }
    (headers, body)
}

async fn write_status(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
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

// The tungstenite handshake callback's `Err` type is large; the mock always answers `Ok`.
#[allow(clippy::result_large_err)]
async fn route(mut stream: TcpStream, shared: Arc<Shared>) {
    // Peek (do not consume) so tungstenite can still read the handshake itself.
    let mut buf = [0_u8; 8192];
    let head = loop {
        let Ok(n) = stream.peek(&mut buf).await else {
            return;
        };
        if n == 0 {
            return;
        }
        if let Some(pos) = find(&buf[..n], b"\r\n\r\n") {
            break String::from_utf8_lossy(&buf[..pos]).to_string();
        }
        if n == buf.len() {
            return;
        }
        tokio::task::yield_now().await;
    };
    let is_ws = head.to_ascii_lowercase().contains("upgrade: websocket");

    if is_ws {
        shared.ws_hits.fetch_add(1, Ordering::SeqCst);
        let handler = shared.ws.lock().unwrap().take();
        let Some(handler) = handler else {
            let _ = read_request(&mut stream).await;
            write_status(&mut stream, 404, "Not Found", "{}").await;
            return;
        };
        let captured = Arc::new(Mutex::new(HashMap::new()));
        let capture = captured.clone();
        let ws = accept_hdr_async(stream, move |req: &Request, resp: Response| {
            let mut map = capture.lock().unwrap();
            for (name, value) in req.headers() {
                map.insert(
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or("").to_string(),
                );
            }
            Ok(resp)
        })
        .await
        .expect("ws handshake");
        let headers = captured.lock().unwrap().clone();
        handler(WsConn { ws, headers }).await;
    } else {
        shared.http_hits.fetch_add(1, Ordering::SeqCst);
        let (headers, body) = read_request(&mut stream).await;
        let handler = shared.http.lock().unwrap().take();
        match handler {
            Some(handler) => {
                handler(HttpReq {
                    headers,
                    body,
                    stream,
                })
                .await
            }
            None => {
                write_status(
                    &mut stream,
                    404,
                    "Not Found",
                    r#"{"status":404,"message":"no route","reason":null}"#,
                )
                .await
            }
        }
    }
}

// ---- wire helpers (MessagePack) -------------------------------------------------------

#[derive(Serialize)]
struct AudioEvent<'a> {
    event: &'a str,
    #[serde(with = "serde_bytes")]
    audio: &'a [u8],
}

#[derive(Serialize)]
struct FinishEvent<'a> {
    event: &'a str,
    reason: &'a str,
}

#[derive(Serialize)]
struct UnknownEvent<'a> {
    event: &'a str,
    detail: u32,
}

async fn send_msgpack<T: Serialize>(ws: &mut WebSocketStream<TcpStream>, value: &T) {
    let bytes = rmp_serde::to_vec_named(value).unwrap();
    // The client may already have closed (cancel test); that is not a server error.
    let _ = ws.send(Message::Binary(bytes.into())).await;
}

async fn send_audio(ws: &mut WebSocketStream<TcpStream>, pcm: &[u8]) {
    send_msgpack(
        ws,
        &AudioEvent {
            event: "audio",
            audio: pcm,
        },
    )
    .await;
}

async fn send_finish(ws: &mut WebSocketStream<TcpStream>, reason: &str) {
    send_msgpack(
        ws,
        &FinishEvent {
            event: "finish",
            reason,
        },
    )
    .await;
}

async fn read_event(ws: &mut WebSocketStream<TcpStream>) -> serde_json::Value {
    loop {
        match ws.next().await.expect("client closed early").unwrap() {
            Message::Binary(bytes) => return rmp_serde::from_slice(&bytes).unwrap(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected client frame: {other:?}"),
        }
    }
}

async fn read_client_events(ws: &mut WebSocketStream<TcpStream>) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    for _ in 0..4 {
        frames.push(read_event(ws).await);
    }
    frames
}

/// Deterministic non-zero 24 kHz mono s16le bytes.
fn pcm(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8) | 1).collect()
}

fn config(endpoint: &str, voice: Option<&str>, model: Option<&str>) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Fishaudio,
        model: model.map(str::to_string),
        model_path: None,
        voice: voice.map(str::to_string),
        api_key: Some("test-key".into()),
        endpoint: Some(endpoint.to_string()),
        sentence_case: Default::default(),
    }
}

fn provider(endpoint: &str, voice: Option<&str>) -> Box<dyn TtsProvider> {
    FishAudioFactory
        .create_tts(&config(endpoint, voice, None))
        .unwrap_or_else(|_| panic!("create_tts"))
}

fn sink() -> (
    TtsProgressiveSink,
    mpsc::UnboundedReceiver<TtsAudioChunk>,
    Arc<AtomicBool>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    (
        TtsProgressiveSink {
            tx,
            cancel: cancel.clone(),
        },
        rx,
        cancel,
    )
}

async fn within<T>(fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("step did not finish within 10 s")
}

fn sorted_keys(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

const REQUEST_KEYS: [&str; 6] = [
    "format",
    "latency",
    "normalize",
    "reference_id",
    "sample_rate",
    "text",
];

// ---- tests ------------------------------------------------------------------------------

#[tokio::test]
async fn ws_streams_chunks_to_the_sink_before_finish() {
    let (captured_tx, captured_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let ws: WsHandler = Box::new(move |mut conn| {
        Box::pin(async move {
            let frames = read_client_events(&mut conn.ws).await;
            let _ = captured_tx.send((conn.headers.clone(), frames));
            send_audio(&mut conn.ws, &pcm(960, 1)).await;
            // The test has the first sink chunk before this resolves: streaming, not buffering.
            let _ = release_rx.await;
            send_msgpack(
                &mut conn.ws,
                &UnknownEvent {
                    event: "log",
                    detail: 7,
                },
            )
            .await;
            send_audio(&mut conn.ws, &pcm(960, 2)).await;
            send_audio(&mut conn.ws, &pcm(960, 3)).await;
            send_finish(&mut conn.ws, "stop").await;
        })
    });
    let mock = Mock::start(Some(ws), None).await;
    let tts = provider(&mock.endpoint(), Some("voice-1"));
    let (sink, mut rx, _cancel) = sink();

    let call = tokio::spawn(async move {
        tts.synthesize_progressive("Hello [warm] world.", Some(sink))
            .await
    });

    let first = within(rx.recv()).await.expect("first sink chunk");
    assert_eq!(first.pcm.len(), 960 * 2 * 2);
    assert!(!call.is_finished(), "first chunk arrived before finish");
    release_tx.send(()).unwrap();

    let returned = within(call).await.unwrap().unwrap();
    let mut sunk = vec![first];
    while let Ok(chunk) = rx.try_recv() {
        sunk.push(chunk);
    }
    assert_eq!(sunk.len(), 3, "each audio event reaches the sink once");
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].pcm.len(), 3 * 960 * 2 * 2);
    let sink_total: Vec<u8> = sunk.iter().flat_map(|c| c.pcm.to_vec()).collect();
    assert_eq!(
        sink_total,
        returned[0].pcm.to_vec(),
        "sink total = returned"
    );
    assert_eq!(
        sunk.iter().map(|c| c.duration_ms).sum::<u32>(),
        returned[0].duration_ms
    );

    let (headers, frames) = within(captured_rx).await.unwrap();
    assert_eq!(headers["authorization"], "Bearer test-key");
    assert_eq!(headers["model"], "drama-3");
    assert_eq!(frames[0]["event"], "start");
    let request = &frames[0]["request"];
    assert_eq!(sorted_keys(request), REQUEST_KEYS);
    assert_eq!(request["text"], "");
    assert_eq!(request["format"], "pcm");
    assert_eq!(request["sample_rate"], 24000);
    assert_eq!(request["latency"], "balanced");
    assert_eq!(request["normalize"], true);
    assert_eq!(request["reference_id"], "voice-1");
    assert_eq!(sorted_keys(&frames[0]), ["event", "request"]);
    assert_eq!(frames[1]["event"], "text");
    assert_eq!(frames[1]["text"], "Hello [warm] world.");
    assert_eq!(frames[2]["event"], "flush");
    assert_eq!(frames[3]["event"], "stop");
    assert_eq!(mock.http_hits(), 0);
}

#[tokio::test]
async fn ws_finish_error_falls_back_to_http() {
    let (captured_tx, captured_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let ws: WsHandler = Box::new(move |mut conn| {
        Box::pin(async move {
            let _ = read_client_events(&mut conn.ws).await;
            send_finish(&mut conn.ws, "error").await;
        })
    });
    let http: HttpHandler = Box::new(move |mut req| {
        Box::pin(async move {
            let _ = captured_tx.send((req.headers.clone(), req.body.clone()));
            write_chunked_head(&mut req.stream).await;
            write_chunk(&mut req.stream, &pcm(960, 1)).await;
            let _ = release_rx.await;
            write_chunk(&mut req.stream, &pcm(960, 2)).await;
            write_chunked_end(&mut req.stream).await;
        })
    });
    let mock = Mock::start(Some(ws), Some(http)).await;
    let tts = provider(&mock.endpoint(), Some("voice-1"));
    let (sink, mut rx, _cancel) = sink();

    let call = tokio::spawn(async move {
        tts.synthesize_progressive("Fallback text.", Some(sink))
            .await
    });

    let first = within(rx.recv()).await.expect("first http chunk");
    assert_eq!(first.pcm.len(), 960 * 2 * 2);
    assert!(
        !call.is_finished(),
        "http chunk streamed before the body ended"
    );
    release_tx.send(()).unwrap();

    let returned = within(call).await.unwrap().unwrap();
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].pcm.len(), 2 * 960 * 2 * 2);
    let mut sunk = vec![first];
    while let Ok(chunk) = rx.try_recv() {
        sunk.push(chunk);
    }
    assert_eq!(sunk.len(), 2);
    assert_eq!(mock.ws_hits(), 1);
    assert_eq!(mock.http_hits(), 1);

    let (headers, body) = within(captured_rx).await.unwrap();
    assert_eq!(headers["model"], "drama-3");
    assert_eq!(headers["authorization"], "Bearer test-key");
    assert_eq!(headers["content-type"], "application/json");
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sorted_keys(&request), REQUEST_KEYS);
    assert_eq!(request["text"], "Fallback text.");
    assert_eq!(request["format"], "pcm");
    assert_eq!(request["sample_rate"], 24000);
    assert_eq!(request["latency"], "balanced");
    assert_eq!(request["normalize"], true);
    assert_eq!(request["reference_id"], "voice-1");
}

#[tokio::test]
async fn ws_error_after_audio_does_not_fall_back() {
    let ws: WsHandler = Box::new(move |mut conn| {
        Box::pin(async move {
            let _ = read_client_events(&mut conn.ws).await;
            send_audio(&mut conn.ws, &pcm(960, 1)).await;
            let _ = conn.ws.close(None).await;
        })
    });
    let http: HttpHandler = Box::new(move |mut req| {
        Box::pin(async move {
            write_status(&mut req.stream, 200, "OK", "").await;
        })
    });
    let mock = Mock::start(Some(ws), Some(http)).await;
    let tts = provider(&mock.endpoint(), Some("voice-1"));
    let (sink, mut rx, _cancel) = sink();

    let err = within(tts.synthesize_progressive("Some text.", Some(sink)))
        .await
        .expect_err("closing without finish after audio is an error");
    match err {
        SpeechError::Vendor { vendor, message } => {
            assert_eq!(vendor, "fishaudio");
            assert!(message.contains("closed before finish"), "{message}");
            assert!(!message.contains("Some text."), "{message}");
        }
        other => panic!("expected vendor error, got {other}"),
    }
    assert!(rx.try_recv().is_ok(), "the first chunk was already played");
    assert_eq!(mock.http_hits(), 0, "no fallback after audio started");
}

/// The clock is paused only around the wait: connecting and the HTTP fallback run on the real
/// clock, because auto-advance fires a connect timeout while loopback I/O is still pending.
///
/// On the current-thread test runtime the client arms its first-audio timer in the same poll
/// that flushes the `stop` frame, before the server task can read that frame. So when the
/// server reports "all frames read", the timer is already armed and `advance` is enough.
#[tokio::test]
async fn ws_that_never_sends_audio_times_out() {
    let (frames_read_tx, frames_read_rx) = oneshot::channel::<()>();
    let ws: WsHandler = Box::new(move |mut conn| {
        Box::pin(async move {
            let _ = read_client_events(&mut conn.ws).await;
            let _ = frames_read_tx.send(());
            // Accept, read everything, answer nothing.
            std::future::pending::<()>().await;
        })
    });
    let mock = Mock::start(Some(ws), None).await;
    let tts = provider(&mock.endpoint(), Some("voice-1"));

    let started = tokio::time::Instant::now();
    let call = tokio::spawn(async move { tts.synthesize("Never answered.").await });

    within(frames_read_rx).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();

    let err = within(call)
        .await
        .unwrap()
        .expect_err("a silent socket must time out");
    let waited = started.elapsed();

    let message = err.to_string();
    assert!(
        message.contains("fishaudio ws first audio timed out after 15000 ms"),
        "{message}"
    );
    assert!(waited >= Duration::from_secs(15), "{waited:?}");
    assert!(waited < Duration::from_secs(25), "{waited:?}");
    assert_eq!(
        mock.http_hits(),
        1,
        "no audio yet, so the HTTP fallback ran"
    );
    assert!(!message.contains("Never answered."), "{message}");
}

#[tokio::test]
async fn http_402_maps_to_a_vendor_error() {
    // No WS handler: the handshake is answered with 404, so the client falls back to HTTP.
    let http: HttpHandler = Box::new(move |mut req| {
        Box::pin(async move {
            write_status(
                &mut req.stream,
                402,
                "Payment Required",
                r#"{"status":402,"message":"No payment","reason":null}"#,
            )
            .await;
        })
    });
    let mock = Mock::start(None, Some(http)).await;
    let tts = provider(&mock.endpoint(), Some("voice-1"));

    let err = within(tts.synthesize("Pay up."))
        .await
        .expect_err("402 is an error");
    match err {
        SpeechError::Vendor { vendor, message } => {
            assert_eq!(vendor, "fishaudio");
            assert!(
                message.contains("fishaudio http 402: No payment"),
                "{message}"
            );
            assert!(!message.contains("Pay up."), "{message}");
        }
        other => panic!("expected vendor error, got {other}"),
    }
    assert_eq!(mock.ws_hits(), 1);
    assert_eq!(mock.http_hits(), 1);
}

#[tokio::test]
async fn cancel_stops_reading_and_closes_the_socket() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (closed_tx, closed_rx) = oneshot::channel::<()>();
    let ws: WsHandler = Box::new(move |mut conn| {
        Box::pin(async move {
            let _ = read_client_events(&mut conn.ws).await;
            send_audio(&mut conn.ws, &pcm(960, 1)).await;
            let _ = go_rx.await;
            send_audio(&mut conn.ws, &pcm(960, 2)).await;
            // The client must close instead of waiting for `finish`.
            loop {
                match conn.ws.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
            let _ = closed_tx.send(());
        })
    });
    let mock = Mock::start(Some(ws), None).await;
    let tts = provider(&mock.endpoint(), Some("voice-1"));
    let (sink, mut rx, cancel) = sink();

    let call = tokio::spawn(async move {
        tts.synthesize_progressive_with_status("Interrupted.", Some(sink))
            .await
    });

    let first = within(rx.recv()).await.expect("first chunk");
    assert_eq!(first.pcm.len(), 960 * 2 * 2);
    cancel.store(true, Ordering::SeqCst);
    go_tx.send(()).unwrap();

    let synthesis = within(call).await.unwrap().unwrap();
    assert!(
        !synthesis.complete,
        "a cancelled synthesis is not cacheable"
    );
    assert_eq!(synthesis.chunks.len(), 1);
    assert_eq!(synthesis.chunks[0].pcm.len(), 960 * 2 * 2);
    within(closed_rx).await.expect("server observed the close");
    assert!(rx.try_recv().is_err(), "nothing sent after cancel");
    assert_eq!(mock.http_hits(), 0, "cancel is not a failure to retry");
}

#[tokio::test]
async fn unknown_model_is_never_sent() {
    let ws: WsHandler = Box::new(|_conn| Box::pin(async {}));
    let http: HttpHandler = Box::new(|_req| Box::pin(async {}));
    let mock = Mock::start(Some(ws), Some(http)).await;

    let result =
        FishAudioFactory.create_tts(&config(&mock.endpoint(), Some("voice-1"), Some("s1-mini")));
    match result {
        Err(SpeechError::Config(message)) => {
            assert!(
                message.contains("s1-mini") && message.contains("drama-3"),
                "{message}"
            );
        }
        Err(other) => panic!("expected Config error, got {other}"),
        Ok(_) => panic!("unknown model must be rejected"),
    }
    assert_eq!(mock.connections(), 0, "nothing reached the server");
}
