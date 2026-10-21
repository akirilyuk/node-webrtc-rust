//! Mock-server tests for the Mistral Voxtral realtime STT provider.
//!
//! A local `TcpListener` accepts WebSocket connections (`accept_hdr_async`, headers and query
//! captured) and hands each one to the test, which then plays the server by hand. The server
//! answers only after it has read the client frame it answers, so ordering comes from the
//! protocol, not from sleeps. `poll_output` never blocks, so `next_output` polls it under a
//! 10 s bound.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttOutput, SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_mistral::{MistralFactory, MistralStt, SttTimeouts};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_hdr_async, WebSocketStream};

type Ws = WebSocketStream<TcpStream>;

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
        Self::start_delayed(Duration::ZERO).await
    }

    /// Every handshake is answered `delay` after the TCP connect.
    async fn start_delayed(delay: Duration) -> Mock {
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
                    tokio::time::sleep(delay).await;
                    let captured =
                        Arc::new(Mutex::new((HashMap::new(), String::new(), String::new())));
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

    /// No further connection arrives within `wait`.
    async fn assert_no_conn(&mut self, wait: Duration) {
        assert!(
            tokio::time::timeout(wait, self.conns.recv()).await.is_err(),
            "unexpected extra connection"
        );
    }
}

async fn within<T>(fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("step did not finish within 10 s")
}

fn config(endpoint: &str, language: Option<&str>) -> SttConfig {
    SttConfig {
        // The Mistral vendor id is added to `SttVendor` with the SDK wiring; the factory
        // ignores the field.
        provider: SttVendor::Mock,
        model: None,
        model_path: None,
        language: language.map(str::to_string),
        api_key: Some("test-key".into()),
        endpoint: Some(endpoint.to_string()),
        transport: None,
    }
}

fn provider(endpoint: &str) -> Box<dyn SttProvider> {
    MistralFactory
        .create_stt(&config(endpoint, None))
        .unwrap_or_else(|_| panic!("create_stt"))
}

/// Starts a provider and takes the pre-opened session's server side, `session.update` read.
async fn started(mock: &mut Mock, stt: &mut Box<dyn SttProvider>) -> (ServerConn, Value) {
    within(stt.start()).await.expect("start");
    let mut conn = mock.next_conn().await;
    let update = read_frame(&mut conn.ws).await;
    (conn, update)
}

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

async fn expect_partial(stt: &mut Box<dyn SttProvider>, text: &str) {
    match next_output(stt).await {
        Ok(SttOutput::Transcript(SttTranscript::Partial(got))) => assert_eq!(got, text),
        other => panic!("expected Partial({text:?}), got {other:?}"),
    }
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

async fn expect_error(stt: &mut Box<dyn SttProvider>, needle: &str) {
    match next_output(stt).await {
        Err(SpeechError::Vendor { vendor, message }) => {
            assert_eq!(vendor, "mistral");
            assert!(message.contains(needle), "`{needle}` in {message}");
        }
        other => panic!("expected a mistral error with `{needle}`, got {other:?}"),
    }
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .expect("server send");
}

async fn delta(ws: &mut Ws, text: &str) {
    send(
        ws,
        json!({"type": "transcription.text.delta", "text": text}),
    )
    .await;
}

async fn done(ws: &mut Ws, text: &str) {
    send(
        ws,
        json!({"type": "transcription.done", "text": text, "language": "en"}),
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

/// Reads appends until `input_audio.end`; returns each decoded append.
async fn read_appends_until_end(ws: &mut Ws) -> Vec<Vec<u8>> {
    let mut appends = Vec::new();
    loop {
        let frame = read_frame(ws).await;
        match frame["type"].as_str().unwrap() {
            "input_audio.append" => {
                assert_eq!(sorted_keys(&frame), ["audio", "type"]);
                appends.push(
                    base64::engine::general_purpose::STANDARD
                        .decode(frame["audio"].as_str().unwrap())
                        .unwrap(),
                );
            }
            "input_audio.end" => {
                assert_eq!(sorted_keys(&frame), ["type"]);
                return appends;
            }
            other => panic!("unexpected client frame type {other}"),
        }
    }
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

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

/// One whole utterance on `conn` (the claimed session): audio, finalize, server answers.
async fn utterance(
    stt: &mut Box<dyn SttProvider>,
    conn: &mut ServerConn,
    text: &str,
    seed: u8,
) -> Vec<u8> {
    let audio = pcm(3200, seed);
    stt.push_audio(audio.clone()).await.unwrap();
    stt.request_finalize().await.unwrap();
    let appends = read_appends_until_end(&mut conn.ws).await;
    assert_eq!(appends.concat(), audio.to_vec());
    delta(&mut conn.ws, text).await;
    done(&mut conn.ws, text).await;
    expect_partial(stt, text).await;
    expect_final(stt, text).await;
    expect_done(stt).await;
    audio.to_vec()
}

// ---- tests -------------------------------------------------------------------------------

#[tokio::test]
async fn two_utterances_use_two_sessions_with_partials_and_finals() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn1, update1) = started(&mut mock, &mut stt).await;
    assert_eq!(update1["type"], "session.update");

    // Utterance 1: partials grow, the done text is the final.
    let audio = pcm(3200, 1);
    stt.push_audio(audio.clone()).await.unwrap();
    stt.request_finalize().await.unwrap();
    assert_eq!(
        read_appends_until_end(&mut conn1.ws).await.concat(),
        audio.to_vec()
    );
    delta(&mut conn1.ws, "Hello").await;
    delta(&mut conn1.ws, " world").await;
    done(&mut conn1.ws, "Hello world").await;
    expect_partial(&mut stt, "Hello").await;
    expect_partial(&mut stt, "Hello world").await;
    expect_final(&mut stt, "Hello world").await;
    expect_done(&mut stt).await;
    // The session of an ended utterance is closed.
    read_until_closed(&mut conn1.ws).await;

    // The next session was opened in the background and gets its own session.update.
    let mut conn2 = mock.next_conn().await;
    let update2 = read_frame(&mut conn2.ws).await;
    assert_eq!(update2["type"], "session.update");
    let audio2 = pcm(3200, 9);
    stt.push_audio(audio2.clone()).await.unwrap();
    stt.request_finalize().await.unwrap();
    assert_eq!(
        read_appends_until_end(&mut conn2.ws).await.concat(),
        audio2.to_vec()
    );
    delta(&mut conn2.ws, "Second").await;
    done(&mut conn2.ws, "Second one").await;
    // Only the second utterance's text: nothing carried over from the first.
    expect_partial(&mut stt, "Second").await;
    expect_final(&mut stt, "Second one").await;
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());
    within(stt.stop()).await.unwrap();
}

#[tokio::test]
async fn session_update_is_sent_once_before_the_first_append() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn, update) = started(&mut mock, &mut stt).await;

    assert_eq!(sorted_keys(&update), ["session", "type"]);
    assert_eq!(update["type"], "session.update");
    let session = &update["session"];
    assert_eq!(
        sorted_keys(session),
        ["audio_format", "target_streaming_delay_ms"]
    );
    assert_eq!(
        sorted_keys(&session["audio_format"]),
        ["encoding", "sample_rate"]
    );
    assert_eq!(session["audio_format"]["encoding"], "pcm_s16le");
    assert_eq!(session["audio_format"]["sample_rate"], 16000);
    assert_eq!(session["target_streaming_delay_ms"], 240);

    // Everything after it is appends and the end; never a second session.update.
    for seed in 0..3 {
        stt.push_audio(pcm(3200, seed)).await.unwrap();
    }
    stt.request_finalize().await.unwrap();
    for frame_type in ["input_audio.append"; 3]
        .into_iter()
        .chain(["input_audio.end"])
    {
        let frame = read_frame(&mut conn.ws).await;
        assert_eq!(frame["type"], frame_type);
    }
}

#[tokio::test]
async fn audio_pushed_while_connecting_is_delivered_in_order() {
    let mut mock = Mock::start_delayed(Duration::from_millis(300)).await;
    let mut stt = provider(&mock.endpoint());
    within(stt.start()).await.unwrap();
    assert!(stt.stream_open_pending(), "the handshake is still pending");

    let parts = [pcm(1000, 1), pcm(2500, 50), pcm(4000, 100), pcm(700, 150)];
    for part in &parts {
        stt.push_audio(part.clone()).await.unwrap();
    }
    stt.request_finalize().await.unwrap();

    let mut conn = mock.next_conn().await;
    let update = read_frame(&mut conn.ws).await;
    assert_eq!(update["type"], "session.update", "before any audio");
    let appends = read_appends_until_end(&mut conn.ws).await;
    let expected: Vec<u8> = parts.iter().flat_map(|p| p.to_vec()).collect();
    assert_eq!(appends.concat(), expected, "nothing dropped, in order");

    done(&mut conn.ws, "ok").await;
    expect_final(&mut stt, "ok").await;
    expect_done(&mut stt).await;
}

#[tokio::test]
async fn appends_are_batched_and_never_exceed_the_max() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn, _) = started(&mut mock, &mut stt).await;

    // 20 frames of 20 ms: five frames reach the 3200-byte target, so four appends.
    let frames: Vec<Bytes> = (0..20).map(|i| pcm(640, i)).collect();
    for frame in &frames {
        stt.push_audio(frame.clone()).await.unwrap();
    }
    stt.request_finalize().await.unwrap();
    let appends = read_appends_until_end(&mut conn.ws).await;
    assert_eq!(
        appends.len(),
        4,
        "{:?}",
        appends.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert!(appends.iter().all(|a| a.len() == 3200));
    let expected: Vec<u8> = frames.iter().flat_map(|f| f.to_vec()).collect();
    assert_eq!(appends.concat(), expected);
    done(&mut conn.ws, "").await;
    expect_done(&mut stt).await;

    // One huge push is split at the documented maximum.
    let mut conn2 = mock.next_conn().await;
    let _ = read_frame(&mut conn2.ws).await;
    let big = pcm(600_000, 7);
    stt.push_audio(big.clone()).await.unwrap();
    stt.request_finalize().await.unwrap();
    let appends = read_appends_until_end(&mut conn2.ws).await;
    assert_eq!(appends.len(), 3);
    assert!(appends.iter().all(|a| a.len() <= 262_144));
    assert_eq!(appends.concat(), big.to_vec());
}

#[tokio::test]
async fn error_event_fails_only_that_utterance() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn1, _) = started(&mut mock, &mut stt).await;

    stt.push_audio(pcm(3200, 1)).await.unwrap();
    stt.request_finalize().await.unwrap();
    let _ = read_appends_until_end(&mut conn1.ws).await;
    send(
        &mut conn1.ws,
        json!({"type": "error", "error": {"message": "rate limited", "code": 429}}),
    )
    .await;
    expect_error(&mut stt, "mistral stt error 429: rate limited").await;
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none(), "one error only");

    // The next utterance works on a new session.
    let mut conn2 = mock.next_conn().await;
    let _ = read_frame(&mut conn2.ws).await;
    utterance(&mut stt, &mut conn2, "Fine", 3).await;
}

#[tokio::test]
async fn server_close_mid_utterance_is_a_named_error_and_next_push_reconnects() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn1, _) = started(&mut mock, &mut stt).await;

    // Close before the finalize: one error, and the later finalize still gets its marker.
    stt.push_audio(pcm(3200, 1)).await.unwrap();
    let _ = read_frame(&mut conn1.ws).await;
    drop(conn1);
    expect_error(&mut stt, "mistral stt websocket closed").await;
    stt.request_finalize().await.unwrap();
    expect_done(&mut stt).await;
    assert!(
        stt.poll_output().await.unwrap().is_none(),
        "exactly one error"
    );

    // The next push reconnects (no socket is open before it).
    stt.push_audio(pcm(3200, 2)).await.unwrap();
    stt.request_finalize().await.unwrap();
    let mut conn2 = mock.next_conn().await;
    assert_eq!(read_frame(&mut conn2.ws).await["type"], "session.update");
    let _ = read_appends_until_end(&mut conn2.ws).await;
    // Close with the finalize pending: the error, then the marker.
    drop(conn2);
    expect_error(&mut stt, "mistral stt websocket closed").await;
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());

    let audio = pcm(3200, 5);
    stt.push_audio(audio.clone()).await.unwrap();
    let mut conn3 = mock.next_conn().await;
    assert_eq!(read_frame(&mut conn3.ws).await["type"], "session.update");
    stt.request_finalize().await.unwrap();
    assert_eq!(
        read_appends_until_end(&mut conn3.ws).await.concat(),
        audio.to_vec()
    );
    delta(&mut conn3.ws, "Back again").await;
    done(&mut conn3.ws, "Back again").await;
    expect_partial(&mut stt, "Back again").await;
    expect_final(&mut stt, "Back again").await;
    expect_done(&mut stt).await;
}

#[tokio::test]
async fn preopened_socket_closed_by_server_reopens_quietly() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (conn1, _) = started(&mut mock, &mut stt).await;
    drop(conn1);

    // The provider notices the close and opens a new socket; no error, no output.
    let mut conn2 = within(async {
        loop {
            let _ = stt.wait_ready(Duration::from_millis(50)).await;
            if let Ok(Some(conn)) =
                tokio::time::timeout(Duration::from_millis(50), mock.conns.recv()).await
            {
                return conn;
            }
        }
    })
    .await;
    let update = read_frame(&mut conn2.ws).await;
    assert_eq!(update["type"], "session.update");
    assert!(stt.poll_output().await.unwrap().is_none(), "quiet");
    utterance(&mut stt, &mut conn2, "After reopen", 4).await;
}

#[tokio::test]
async fn done_that_never_arrives_times_out() {
    let mut mock = Mock::start().await;
    let mut stt = MistralStt::with_timeouts(
        &config(&mock.endpoint(), None),
        SttTimeouts {
            done: Duration::from_millis(200),
            ..SttTimeouts::default()
        },
    )
    .map(|s| Box::new(s) as Box<dyn SttProvider>)
    .unwrap();
    let (mut conn, _) = started(&mut mock, &mut stt).await;

    stt.push_audio(pcm(3200, 1)).await.unwrap();
    stt.request_finalize().await.unwrap();
    let _ = read_appends_until_end(&mut conn.ws).await;
    // The server answers nothing.
    let started_at = std::time::Instant::now();
    expect_error(
        &mut stt,
        "mistral stt transcription.done timed out after 200 ms",
    )
    .await;
    expect_done(&mut stt).await;
    assert!(started_at.elapsed() < Duration::from_secs(5));
    read_until_closed(&mut conn.ws).await;
}

#[tokio::test]
async fn ping_between_turns_does_not_end_the_reader() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn1, _) = started(&mut mock, &mut stt).await;
    // A ping on the idle, pre-opened socket, then the utterance runs on it.
    conn1.ws.send(Message::Ping(vec![1].into())).await.unwrap();
    stt.push_audio(pcm(3200, 1)).await.unwrap();
    stt.request_finalize().await.unwrap();
    let _ = read_appends_until_end(&mut conn1.ws).await;
    conn1.ws.send(Message::Ping(vec![2].into())).await.unwrap();
    delta(&mut conn1.ws, "one").await;
    conn1.ws.send(Message::Pong(vec![3].into())).await.unwrap();
    done(&mut conn1.ws, "one").await;
    expect_partial(&mut stt, "one").await;
    expect_final(&mut stt, "one").await;
    expect_done(&mut stt).await;

    let mut conn2 = mock.next_conn().await;
    let _ = read_frame(&mut conn2.ws).await;
    conn2.ws.send(Message::Ping(vec![4].into())).await.unwrap();
    utterance(&mut stt, &mut conn2, "two", 2).await;
}

#[tokio::test]
async fn two_letter_language_is_sent_and_region_tag_is_omitted() {
    for (language, expected) in [
        (Some("en"), Some("en")),
        (Some("DE"), Some("de")),
        (Some("en-US"), None),
        (Some("multi"), None),
        (None, None),
    ] {
        let mut mock = Mock::start().await;
        let mut stt = MistralFactory
            .create_stt(&config(&mock.endpoint(), language))
            .unwrap_or_else(|_| panic!("create_stt"));
        let (_conn, update) = started(&mut mock, &mut stt).await;
        let session = update["session"].as_object().unwrap();
        assert_eq!(
            session.get("language").and_then(Value::as_str),
            expected,
            "{language:?}"
        );
        if expected.is_none() {
            assert!(!session.contains_key("language"), "{language:?}");
        }
    }
}

#[tokio::test]
async fn finalize_without_audio_answers_empty_without_network() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn, _) = started(&mut mock, &mut stt).await;

    stt.request_finalize().await.unwrap();
    stt.request_finalize().await.unwrap();
    expect_done(&mut stt).await;
    expect_done(&mut stt).await;
    assert!(stt.poll_output().await.unwrap().is_none());
    // The socket saw nothing after the session.update, and no new socket was opened.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), conn.ws.next())
            .await
            .is_err(),
        "no frame after session.update"
    );
    mock.assert_no_conn(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn unknown_events_are_ignored() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    let (mut conn, _) = started(&mut mock, &mut stt).await;

    stt.push_audio(pcm(3200, 1)).await.unwrap();
    stt.request_finalize().await.unwrap();
    let _ = read_appends_until_end(&mut conn.ws).await;
    send(
        &mut conn.ws,
        json!({"type": "session.created", "session": {}}),
    )
    .await;
    send(&mut conn.ws, json!({"type": "session.updated"})).await;
    send(
        &mut conn.ws,
        json!({"type": "transcription.language", "language": "en"}),
    )
    .await;
    send(
        &mut conn.ws,
        json!({"type": "transcription.segment", "text": "x"}),
    )
    .await;
    send(&mut conn.ws, json!({"type": "something.new", "x": 1})).await;
    conn.ws
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    conn.ws
        .send(Message::Text("not json".to_string().into()))
        .await
        .unwrap();
    delta(&mut conn.ws, "still works").await;
    done(&mut conn.ws, "still works").await;
    expect_partial(&mut stt, "still works").await;
    expect_final(&mut stt, "still works").await;
    expect_done(&mut stt).await;
}

#[tokio::test]
async fn missing_api_key_is_a_named_error() {
    let mut mock = Mock::start().await;
    std::env::remove_var("MISTRAL_API_KEY");
    let mut cfg = config(&mock.endpoint(), None);
    cfg.api_key = None;
    let mut stt = MistralFactory
        .create_stt(&cfg)
        .unwrap_or_else(|_| panic!("create_stt"));
    let err = within(stt.start()).await.expect_err("no key");
    assert!(err.to_string().contains("missing MISTRAL_API_KEY"), "{err}");
    mock.assert_no_conn(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn unknown_model_is_rejected() {
    let mut mock = Mock::start().await;
    for model in [
        "voxtral-mini-transcribe-realtime-latest",
        "voxtral-mini-transcribe-2",
        "",
    ] {
        let mut cfg = config(&mock.endpoint(), None);
        cfg.model = Some(model.to_string());
        match MistralFactory.create_stt(&cfg) {
            Err(SpeechError::Config(message)) => assert!(
                message.contains(model) && message.contains("voxtral-mini-transcribe-realtime-3-0"),
                "{message}"
            ),
            Err(other) => panic!("expected a config error, got {other}"),
            Ok(_) => panic!("`{model}` must be rejected"),
        }
    }
    mock.assert_no_conn(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn bearer_header_and_model_query_are_sent() {
    let mut mock = Mock::start().await;
    let mut stt = provider(&mock.endpoint());
    within(stt.start()).await.unwrap();
    let conn = mock.next_conn().await;
    assert_eq!(conn.path, "/v1/audio/transcriptions/realtime");
    assert_eq!(
        conn.headers.get("authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    assert_eq!(conn.query["model"], "voxtral-mini-transcribe-realtime-3-0");
    assert_eq!(
        conn.query.len(),
        1,
        "no other query field: {:?}",
        conn.query
    );
    assert_eq!(stt.vendor_name(), "mistral");
    assert!(stt.wait_ready(Duration::from_secs(10)).await.unwrap());

    let mut cfg = config(&mock.endpoint(), None);
    cfg.model = Some("voxtral-mini-transcribe-realtime-3".into());
    let mut alias = MistralFactory
        .create_stt(&cfg)
        .unwrap_or_else(|_| panic!("create_stt"));
    within(alias.start()).await.unwrap();
    let conn = mock.next_conn().await;
    assert_eq!(conn.query["model"], "voxtral-mini-transcribe-realtime-3");
    within(alias.stop()).await.unwrap();
    within(stt.stop()).await.unwrap();
    assert_eq!(stt.last_transport().as_deref(), Some("websocket"));
}
