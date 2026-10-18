//! Mock-server tests for the Fish Audio realtime STT provider. Run with `--features live`.
//!
//! A local `TcpListener` accepts WebSocket connections (`accept_hdr_async`, headers and query
//! captured) and hands each one to the test, which then plays the server by hand. The server
//! only answers after it has read the client frame it answers, so ordering comes from the
//! protocol, not from sleeps. The provider's `poll_output` never blocks, so `next_output`
//! polls it under a 10 s bound. The paused clock is used only around the waits that need it.
#![cfg(feature = "live")]

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttOutput, SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_fishaudio::FishAudioFactory;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_hdr_async, WebSocketStream};

type Ws = WebSocketStream<TcpStream>;

const PATH: &str = "/compat/elevenlabs/v1/speech-to-text/realtime";

struct ServerConn {
    ws: Ws,
    headers: HashMap<String, String>,
    path: String,
    query: HashMap<String, String>,
}

struct Mock {
    port: u16,
    conns: mpsc::UnboundedReceiver<ServerConn>,
    accept: tokio::task::JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl Mock {
    async fn start() -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, conns) = mpsc::unbounded_channel();
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let captured = std::sync::Arc::new(std::sync::Mutex::new((
                        HashMap::new(),
                        String::new(),
                        String::new(),
                    )));
                    let capture = captured.clone();
                    // The handshake callback's `Err` type is large; the mock always answers `Ok`.
                    #[allow(clippy::result_large_err)]
                    let callback = move |req: &Request, resp: Response| {
                        let mut captured = capture.lock().unwrap();
                        for (name, value) in req.headers() {
                            captured.0.insert(
                                name.as_str().to_ascii_lowercase(),
                                value.to_str().unwrap_or("").to_string(),
                            );
                        }
                        captured.1 = req.uri().path().to_string();
                        captured.2 = req.uri().query().unwrap_or("").to_string();
                        Ok(resp)
                    };
                    let Ok(ws) = accept_hdr_async(stream, callback).await else {
                        return;
                    };
                    let (headers, path, query) = captured.lock().unwrap().clone();
                    let query = query
                        .split('&')
                        .filter(|pair| !pair.is_empty())
                        .map(|pair| {
                            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                            (key.to_string(), value.to_string())
                        })
                        .collect();
                    let _ = tx.send(ServerConn {
                        ws,
                        headers,
                        path,
                        query,
                    });
                });
            }
        });
        Mock {
            port,
            conns,
            accept,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn next_conn(&mut self) -> ServerConn {
        within(self.conns.recv()).await.expect("mock stopped")
    }
}

async fn within<T>(fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("step did not finish within 10 s")
}

fn config(endpoint: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::Fishaudio,
        model: None,
        model_path: None,
        language: None,
        api_key: Some("test-key".into()),
        endpoint: Some(endpoint.to_string()),
        transport: None,
    }
}

fn provider(endpoint: &str) -> Box<dyn SttProvider> {
    FishAudioFactory
        .create_stt(&config(endpoint))
        .unwrap_or_else(|_| panic!("create_stt"))
}

/// Starts a provider against the mock, answers `session_started` and waits for ready.
async fn ready_provider(mock: &mut Mock) -> (Box<dyn SttProvider>, ServerConn) {
    let mut stt = provider(&mock.endpoint());
    within(stt.start()).await.expect("start");
    let mut conn = mock.next_conn().await;
    send(
        &mut conn.ws,
        json!({"message_type": "session_started", "session_id": "s-1"}),
    )
    .await;
    assert!(within(stt.wait_ready(Duration::from_secs(10)))
        .await
        .unwrap());
    (stt, conn)
}

/// Polls `poll_output` until it yields an item or an error (the real clock, 10 s bound).
async fn next_output(stt: &mut Box<dyn SttProvider>) -> SpeechResult<SttOutput> {
    within(async {
        loop {
            if let Some(output) = stt.poll_output().await? {
                return Ok(output);
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
}

async fn expect_final(stt: &mut Box<dyn SttProvider>, text: &str) {
    match next_output(stt).await {
        Ok(SttOutput::Transcript(SttTranscript::Final(got))) => assert_eq!(got, text),
        other => panic!("expected Final({text:?}), got {other:?}"),
    }
}

async fn expect_done(stt: &mut Box<dyn SttProvider>) {
    match next_output(stt).await {
        Ok(SttOutput::FinalizeDone) => {}
        other => panic!("expected FinalizeDone, got {other:?}"),
    }
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .expect("server send");
}

async fn committed(ws: &mut Ws, text: &str) {
    send(
        ws,
        json!({"message_type": "committed_transcript", "text": text}),
    )
    .await;
}

/// Next client text frame as JSON (pings and pongs skipped).
async fn read_frame(ws: &mut Ws) -> Value {
    within(async {
        loop {
            match ws.next().await.expect("client closed early").unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(_) | Message::Pong(_) => {}
                other => panic!("unexpected client frame: {other:?}"),
            }
        }
    })
    .await
}

fn decode_audio(frame: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(frame["audio_base_64"].as_str().unwrap())
        .unwrap()
}

fn assert_audio_frame(frame: &Value, expected: &[u8]) {
    assert_eq!(
        sorted_keys(frame),
        ["audio_base_64", "commit", "message_type", "sample_rate"]
    );
    assert_eq!(frame["message_type"], "input_audio_chunk");
    assert_eq!(frame["commit"], false);
    assert_eq!(frame["sample_rate"], 16000);
    assert_eq!(decode_audio(frame), expected);
}

fn assert_commit_frame(frame: &Value) {
    assert_eq!(
        sorted_keys(frame),
        ["audio_base_64", "commit", "message_type", "sample_rate"]
    );
    assert_eq!(frame["message_type"], "input_audio_chunk");
    assert_eq!(frame["audio_base_64"], "");
    assert_eq!(frame["commit"], true);
    assert_eq!(frame["sample_rate"], 16000);
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

/// Reads until the server side sees the socket end (close frame, EOF or reset).
async fn read_until_closed(ws: &mut Ws) {
    within(async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
}

/// Deterministic non-zero 16 kHz mono s16le bytes.
fn pcm(len: usize, seed: u8) -> Bytes {
    Bytes::from(
        (0..len)
            .map(|i| seed.wrapping_add(i as u8) | 1)
            .collect::<Vec<u8>>(),
    )
}

// ---- handshake ----------------------------------------------------------------------------

#[tokio::test]
async fn handshake_carries_key_and_documented_query() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    within(stt.start()).await.expect("start");
    let mut conn = mock.next_conn().await;

    assert_eq!(conn.path, PATH);
    assert_eq!(
        conn.headers.get("xi-api-key").map(String::as_str),
        Some("test-key")
    );
    assert_eq!(conn.query["model_id"], "fish-audio/transcribe-1");
    assert_eq!(conn.query["commit_strategy"], "manual");
    assert_eq!(conn.query["audio_format"], "pcm_16000");
    assert_eq!(
        conn.query.len(),
        3,
        "no other query field: {:?}",
        conn.query
    );

    // The open is pending until the server says `session_started`.
    assert!(stt.stream_open_pending());
    send(&mut conn.ws, json!({"message_type": "session_started"})).await;
    assert!(within(stt.wait_ready(Duration::from_secs(10)))
        .await
        .unwrap());
    assert!(!stt.stream_open_pending());
}

#[tokio::test]
async fn connect_failure_is_a_named_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut stt = provider(&format!("http://127.0.0.1:{port}"));
    let err = within(stt.start()).await.expect_err("nothing listens");
    match err {
        SpeechError::Vendor { vendor, message } => {
            assert_eq!(vendor, "fishaudio");
            assert!(
                message.contains("fishaudio stt connect failed"),
                "{message}"
            );
            assert!(!message.contains("test-key"), "{message}");
        }
        other => panic!("expected vendor error, got {other}"),
    }
    assert!(!stt.stream_open_pending());
}

// ---- audio and commit -----------------------------------------------------------------------

#[tokio::test]
async fn pushed_audio_arrives_as_base64_in_order_with_commit_false() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    let chunks: Vec<Bytes> = (0..50).map(|i| pcm(320 + i, i as u8)).collect();
    for chunk in &chunks {
        within(stt.push_audio(chunk.clone())).await.unwrap();
    }
    for chunk in &chunks {
        let frame = read_frame(&mut conn.ws).await;
        assert_audio_frame(&frame, chunk);
    }
}

#[tokio::test]
async fn commit_yields_final_then_finalize_done() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    let audio = pcm(640, 3);
    within(stt.push_audio(audio.clone())).await.unwrap();
    within(stt.request_finalize()).await.unwrap();
    assert_audio_frame(&read_frame(&mut conn.ws).await, &audio);
    assert_commit_frame(&read_frame(&mut conn.ws).await);

    // `committed_transcript_with_timestamps` and unknown types produce nothing.
    committed(&mut conn.ws, "hello world").await;
    send(
        &mut conn.ws,
        json!({"message_type": "committed_transcript_with_timestamps", "text": "hello world", "words": []}),
    )
    .await;
    send(&mut conn.ws, json!({"message_type": "something_new"})).await;
    expect_final(&mut stt, "hello world").await;
    expect_done(&mut stt).await;

    // A second commit proves the ignored frames queued nothing in between.
    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    committed(&mut conn.ws, "second").await;
    expect_final(&mut stt, "second").await;
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());
}

#[tokio::test]
async fn empty_committed_text_yields_only_finalize_done() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    committed(&mut conn.ws, "").await;
    expect_done(&mut stt).await;

    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    committed(&mut conn.ws, "   ").await;
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());
}

#[tokio::test]
async fn two_commits_are_answered_in_order() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    within(stt.push_audio(pcm(320, 1))).await.unwrap();
    within(stt.request_finalize()).await.unwrap();
    within(stt.push_audio(pcm(320, 2))).await.unwrap();
    within(stt.request_finalize()).await.unwrap();
    let frames = [
        read_frame(&mut conn.ws).await,
        read_frame(&mut conn.ws).await,
        read_frame(&mut conn.ws).await,
        read_frame(&mut conn.ws).await,
    ];
    assert_audio_frame(&frames[0], &pcm(320, 1));
    assert_commit_frame(&frames[1]);
    assert_audio_frame(&frames[2], &pcm(320, 2));
    assert_commit_frame(&frames[3]);

    committed(&mut conn.ws, "one").await;
    committed(&mut conn.ws, "two").await;
    expect_final(&mut stt, "one").await;
    expect_done(&mut stt).await;
    expect_final(&mut stt, "two").await;
    expect_done(&mut stt).await;
}

#[tokio::test]
async fn input_error_during_a_pending_commit_is_an_error_then_finalize_done() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    send(
        &mut conn.ws,
        json!({"message_type": "input_error", "message": "bad chunk"}),
    )
    .await;

    let err = next_output(&mut stt).await.expect_err("error first");
    match err {
        SpeechError::Vendor { vendor, message } => {
            assert_eq!(vendor, "fishaudio");
            assert_eq!(message, "bad chunk");
        }
        other => panic!("expected vendor error, got {other}"),
    }
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());
}

// ---- socket close and reconnect -------------------------------------------------------------

#[tokio::test]
async fn close_with_a_commit_pending_errors_answers_it_and_the_next_push_reconnects() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    within(stt.push_audio(pcm(320, 7))).await.unwrap();
    within(stt.request_finalize()).await.unwrap();
    assert_audio_frame(&read_frame(&mut conn.ws).await, &pcm(320, 7));
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    drop(conn); // the server closes with the commit unanswered

    let err = next_output(&mut stt).await.expect_err("close is an error");
    assert!(err.to_string().contains("fishaudio stt socket"), "{err}");
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());

    // The next push reconnects: a second handshake with the same query and key.
    within(stt.push_audio(pcm(320, 9))).await.unwrap();
    let mut second = mock.next_conn().await;
    assert_eq!(second.path, PATH);
    assert_eq!(
        second.headers.get("xi-api-key").map(String::as_str),
        Some("test-key")
    );
    assert_eq!(second.query["commit_strategy"], "manual");
    assert_audio_frame(&read_frame(&mut second.ws).await, &pcm(320, 9));
    send(&mut second.ws, json!({"message_type": "session_started"})).await;
    assert!(within(stt.wait_ready(Duration::from_secs(10)))
        .await
        .unwrap());

    // And it works end to end on the new socket.
    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut second.ws).await);
    committed(&mut second.ws, "again").await;
    expect_final(&mut stt, "again").await;
    expect_done(&mut stt).await;
}

/// Nothing was lost: no commit pending, no audio since the last commit. The agent would turn
/// an error here into an STT error during silence, so the close only marks the socket dead.
#[tokio::test]
async fn idle_close_without_pending_audio_is_not_an_error() {
    let mut mock = Mock::start().await;
    let (mut stt, conn) = ready_provider(&mut mock).await;
    drop(conn);

    // 500 ms of real time: only `None`, never an error.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        assert!(stt.poll_output().await.unwrap().is_none());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // The next push reconnects, and the new socket works end to end.
    within(stt.push_audio(pcm(320, 4))).await.unwrap();
    let mut second = mock.next_conn().await;
    within(stt.request_finalize()).await.unwrap();
    assert_audio_frame(&read_frame(&mut second.ws).await, &pcm(320, 4));
    assert_commit_frame(&read_frame(&mut second.ws).await);
    committed(&mut second.ws, "back again").await;
    expect_final(&mut stt, "back again").await;
    expect_done(&mut stt).await;
}

#[tokio::test]
async fn close_with_uncommitted_audio_surfaces_one_error() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    for seed in 0..3 {
        within(stt.push_audio(pcm(320, seed))).await.unwrap();
    }
    for seed in 0..3 {
        assert_audio_frame(&read_frame(&mut conn.ws).await, &pcm(320, seed));
    }
    drop(conn); // closes before any commit: the three chunks are lost

    let err = next_output(&mut stt)
        .await
        .expect_err("lost audio is an error");
    assert!(err.to_string().contains("fishaudio stt socket"), "{err}");
    // Exactly one error, then nothing.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    while tokio::time::Instant::now() < deadline {
        assert!(stt.poll_output().await.unwrap().is_none());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn reconnect_after_close_sends_no_stale_audio() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    // Audio sent on the first socket, then the server hangs up with nothing pending.
    within(stt.push_audio(pcm(320, 1))).await.unwrap();
    assert_audio_frame(&read_frame(&mut conn.ws).await, &pcm(320, 1));
    drop(conn);
    let err = next_output(&mut stt)
        .await
        .expect_err("close is surfaced once");
    assert!(err.to_string().contains("fishaudio stt socket"), "{err}");
    assert!(stt.poll_output().await.unwrap().is_none());

    // The new socket carries only what is pushed after the reconnect.
    within(stt.push_audio(pcm(320, 2))).await.unwrap();
    let mut second = mock.next_conn().await;
    within(stt.request_finalize()).await.unwrap();
    assert_audio_frame(&read_frame(&mut second.ws).await, &pcm(320, 2));
    assert_commit_frame(&read_frame(&mut second.ws).await);
}

/// One connection lives at most 3600 s: at 3300 s of age and an idle moment (nothing pending,
/// no audio waiting for its commit) the next push moves to a fresh socket and closes the old.
///
/// The clock is paused only around the jump. Connecting runs on the real clock, because
/// auto-advance fires a connect timeout while loopback I/O is still pending.
#[tokio::test]
async fn idle_connection_is_replaced_after_3300_seconds() {
    let mut mock = Mock::start().await;
    let (mut stt, mut first) = ready_provider(&mut mock).await;

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3301)).await;
    tokio::time::resume();

    within(stt.push_audio(pcm(320, 5))).await.unwrap();
    let mut second = mock.next_conn().await;
    assert_audio_frame(&read_frame(&mut second.ws).await, &pcm(320, 5));
    read_until_closed(&mut first.ws).await;
}

#[tokio::test]
async fn connection_is_kept_after_3300_seconds_while_an_utterance_is_in_progress() {
    let mut mock = Mock::start().await;
    let (mut stt, mut first) = ready_provider(&mut mock).await;

    // Audio is waiting for its commit, so replacing the socket would split the utterance.
    within(stt.push_audio(pcm(320, 1))).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3301)).await;
    tokio::time::resume();
    within(stt.push_audio(pcm(320, 2))).await.unwrap();
    within(stt.request_finalize()).await.unwrap();

    assert_audio_frame(&read_frame(&mut first.ws).await, &pcm(320, 1));
    assert_audio_frame(&read_frame(&mut first.ws).await, &pcm(320, 2));
    assert_commit_frame(&read_frame(&mut first.ws).await);
    assert!(mock.conns.try_recv().is_err(), "no second handshake");
}

// ---- readiness, discard, stop ---------------------------------------------------------------

/// `session_started` never comes. The clock is paused only around the wait: the spawned task
/// arms its timer on the first poll, so after a few yields `advance` is enough.
#[tokio::test]
async fn wait_ready_is_false_when_session_started_never_comes() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    within(stt.start()).await.expect("start");
    let _conn = mock.next_conn().await; // accepted, never says `session_started`

    let waiter = tokio::spawn(async move {
        let ready = stt.wait_ready(Duration::from_secs(5)).await;
        (stt, ready)
    });
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::time::resume();

    let (stt, ready) = within(waiter).await.unwrap();
    assert!(!ready.unwrap());
    assert!(stt.stream_open_pending(), "the open is still pending");
}

#[tokio::test]
async fn discard_drops_a_queued_final_and_keeps_the_marker() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    committed(&mut conn.ws, "stale words").await;

    // The final and its marker are queued together; wait until the final is there to drop.
    let dropped = within(async {
        loop {
            let dropped = stt.discard_queued_transcripts();
            if dropped > 0 {
                return dropped;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert_eq!(dropped, 1);
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());
}

#[tokio::test]
async fn stop_closes_without_waiting_for_the_server() {
    let mut mock = Mock::start().await;
    let (mut stt, mut conn) = ready_provider(&mut mock).await;

    // A commit is pending and the server never answers; stop must still return.
    within(stt.request_finalize()).await.unwrap();
    assert_commit_frame(&read_frame(&mut conn.ws).await);
    within(stt.stop()).await.unwrap();

    read_until_closed(&mut conn.ws).await;
    // Stopped: pushes are ignored and nothing reconnects.
    within(stt.push_audio(pcm(320, 1))).await.unwrap();
    assert!(stt.poll_output().await.unwrap().is_none());
    assert!(mock.conns.try_recv().is_err(), "no reconnect after stop");
}
