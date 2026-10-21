//! Mock-server tests for the Mistral Voxtral TTS client.
//!
//! One local `TcpListener` answers `POST /v1/audio/speech` with a scripted server-sent-event
//! body. Every request is recorded, so tests can assert the exact request and that no second
//! request (retry or fallback) was made. Ordering uses the socket itself, never sleeps.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pcm::MonoToStereo48k;
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use node_webrtc_rust_vendor_mistral::{MistralTts, Timeouts};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

type BoxFut = Pin<Box<dyn Future<Output = ()> + Send>>;
type Handler = Arc<dyn Fn(TcpStream) -> BoxFut + Send + Sync>;

#[derive(Clone, Debug)]
struct Recorded {
    request_line: String,
    headers: std::collections::HashMap<String, String>,
    body: serde_json::Value,
}

struct Mock {
    port: u16,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Recorded>>>,
    accept: tokio::task::JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl Mock {
    async fn start<F, Fut>(handler: F) -> Mock
    where
        F: Fn(TcpStream) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |s| Box::pin(handler(s)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (h, r) = (hits.clone(), requests.clone());
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (handler, h, r) = (handler.clone(), h.clone(), r.clone());
                tokio::spawn(async move {
                    let Some(recorded) = read_request(&mut stream).await else {
                        return;
                    };
                    h.fetch_add(1, Ordering::SeqCst);
                    r.lock().unwrap().push(recorded);
                    handler(stream).await;
                });
            }
        });
        Mock {
            port,
            hits,
            requests,
            accept,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn request(&self, index: usize) -> Recorded {
        self.requests.lock().unwrap()[index].clone()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let request_line = head.lines().next().unwrap_or("").to_string();
    let headers: std::collections::HashMap<String, String> = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        request_line,
        headers,
        body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    })
}

async fn write_sse_head(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
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

async fn write_event(stream: &mut TcpStream, value: &serde_json::Value) {
    write_chunk(stream, format!("data: {value}\n\n").as_bytes()).await;
}

async fn write_delta(stream: &mut TcpStream, bytes: &[u8]) {
    let audio_data = base64::engine::general_purpose::STANDARD.encode(bytes);
    write_event(
        stream,
        &json!({"type": "speech.audio.delta", "audio_data": audio_data}),
    )
    .await;
}

async fn write_done(stream: &mut TcpStream) {
    write_event(
        stream,
        &json!({"type": "speech.audio.done", "usage": {"prompt_tokens": 1, "total_tokens": 1}}),
    )
    .await;
}

async fn end_chunked(stream: &mut TcpStream) {
    stream.write_all(b"0\r\n\r\n").await.unwrap();
    let _ = stream.shutdown().await;
}

async fn write_status(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
    let _ = stream.shutdown().await;
}

/// Serves `deltas` then `speech.audio.done`.
async fn serve_deltas(mut stream: TcpStream, deltas: Vec<Vec<u8>>) {
    write_sse_head(&mut stream).await;
    for delta in &deltas {
        write_delta(&mut stream, delta).await;
    }
    write_done(&mut stream).await;
    end_chunked(&mut stream).await;
}

// ---- WAV and PCM helpers -----------------------------------------------------------------

fn wav_header(tag: u16, bits: u16, rate: u32, data_size: u32, extra_chunk: &[u8]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&tag.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * u32::from(bits / 8)).to_le_bytes());
    h.extend_from_slice(&(bits / 8).to_le_bytes());
    h.extend_from_slice(&bits.to_le_bytes());
    h.extend_from_slice(extra_chunk);
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_size.to_le_bytes());
    h
}

/// Deterministic non-zero samples.
fn samples(len: usize) -> Vec<i16> {
    (0..len)
        .map(|i| ((i as i32 * 37 + 11) % 20_000 - 10_000) as i16)
        .collect()
}

fn s16_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// What the client must produce for these 24 kHz mono samples.
fn expected_stereo(samples: &[i16]) -> Vec<u8> {
    let mut up = MonoToStereo48k::new(24_000).unwrap();
    let mut out = up.push(&s16_bytes(samples));
    out.extend(up.finish());
    out
}

// ---- provider helpers --------------------------------------------------------------------

fn config(endpoint: &str, voice: Option<&str>, model: Option<&str>) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Mistral,
        model: model.map(str::to_string),
        model_path: None,
        voice: voice.map(str::to_string),
        api_key: Some("test-key".into()),
        endpoint: Some(endpoint.to_string()),
        sentence_case: Default::default(),
        transport: None,
    }
}

fn provider(endpoint: &str) -> MistralTts {
    MistralTts::new(&config(endpoint, None, None)).unwrap()
}

fn sink() -> (TtsProgressiveSink, mpsc::UnboundedReceiver<TtsAudioChunk>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        TtsProgressiveSink {
            tx,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        rx,
    )
}

fn drain(rx: &mut mpsc::UnboundedReceiver<TtsAudioChunk>) -> Vec<TtsAudioChunk> {
    let mut chunks = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        chunks.push(chunk);
    }
    chunks
}

async fn within<T>(fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .expect("test step did not finish in time")
}

fn concat(chunks: &[TtsAudioChunk]) -> Vec<u8> {
    chunks.iter().flat_map(|c| c.pcm.to_vec()).collect()
}

/// Runs one synthesis against `deltas` and checks sink bytes == returned bytes == `expected`.
async fn assert_plays_once(deltas: Vec<Vec<u8>>, expected: Vec<u8>) -> usize {
    let mock = Mock::start(move |stream| serve_deltas(stream, deltas.clone())).await;
    let tts = provider(&mock.endpoint());
    let (sink, mut rx) = sink();

    let returned = within(tts.synthesize_progressive("Hello there.", Some(sink)))
        .await
        .unwrap();
    let streamed = drain(&mut rx);

    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].pcm.to_vec(), expected, "returned buffer");
    assert_eq!(concat(&streamed), expected, "sink bytes (exactly once)");
    assert_eq!(mock.hits(), 1);
    streamed.len()
}

// ---- tests -------------------------------------------------------------------------------

#[tokio::test]
async fn each_delta_reaches_the_sink_once() {
    let pcm = samples(1200);
    let data = s16_bytes(&pcm);
    let mut first = wav_header(1, 16, 24_000, 0, &[]);
    // Delta boundaries sit on odd byte offsets to exercise the carry.
    first.extend_from_slice(&data[..301]);
    let deltas = vec![first, data[301..1003].to_vec(), data[1003..].to_vec()];

    let chunks = assert_plays_once(deltas, expected_stereo(&pcm)).await;
    // Three deltas plus the interpolator tail: never more chunks than there were writes.
    assert!((3..=4).contains(&chunks), "{chunks} sink chunks");
}

#[tokio::test]
async fn wav_header_split_across_deltas_is_never_played() {
    let pcm = samples(600);
    let mut wav = wav_header(1, 16, 24_000, 0, &[]);
    wav.extend_from_slice(&s16_bytes(&pcm));
    // 10 bytes, 30 bytes, the rest: the split falls inside the RIFF header, inside the fmt
    // chunk and across the `data` chunk header.
    let deltas = vec![wav[..10].to_vec(), wav[10..40].to_vec(), wav[40..].to_vec()];
    assert_plays_once(deltas, expected_stereo(&pcm)).await;
}

#[tokio::test]
async fn float32_wav_is_decoded() {
    let pcm = samples(500);
    let floats: Vec<f32> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
    let mut wav = wav_header(3, 32, 24_000, 0xFFFF_FFFF, &[]);
    for f in &floats {
        wav.extend_from_slice(&f.to_le_bytes());
    }
    // What the client must emit: each float scaled and rounded to s16.
    let expected_s16: Vec<i16> = floats
        .iter()
        .map(|f| (f.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect();
    let deltas = vec![wav[..50].to_vec(), wav[50..].to_vec()];
    assert_plays_once(deltas, expected_stereo(&expected_s16)).await;
}

#[tokio::test]
async fn extra_chunk_before_data_is_skipped() {
    let pcm = samples(300);
    // A LIST chunk of odd size (5) with its pad byte.
    let mut list = Vec::new();
    list.extend_from_slice(b"LIST");
    list.extend_from_slice(&5u32.to_le_bytes());
    list.extend_from_slice(b"INFO!");
    list.push(0);
    let mut wav = wav_header(1, 16, 24_000, 0, &list);
    wav.extend_from_slice(&s16_bytes(&pcm));
    let deltas = vec![wav[..48].to_vec(), wav[48..].to_vec()];
    assert_plays_once(deltas, expected_stereo(&pcm)).await;
}

#[tokio::test]
async fn stream_closed_before_done_is_an_error() {
    let pcm = samples(300);
    let mock = Mock::start(move |mut stream| {
        let mut wav = wav_header(1, 16, 24_000, 0, &[]);
        wav.extend_from_slice(&s16_bytes(&pcm));
        async move {
            write_sse_head(&mut stream).await;
            write_delta(&mut stream, &wav).await;
            // Clean close, but no `speech.audio.done`.
            end_chunked(&mut stream).await;
        }
    })
    .await;
    let tts = provider(&mock.endpoint());
    let (sink, mut rx) = sink();

    let err = within(tts.synthesize_progressive("Hello there.", Some(sink)))
        .await
        .expect_err("a stream without speech.audio.done is not a success");
    assert!(
        err.to_string()
            .contains("mistral tts stream ended before speech.audio.done"),
        "{err}"
    );
    assert!(
        !drain(&mut rx).is_empty(),
        "the delta before the close was played"
    );
    assert_eq!(mock.hits(), 1, "no retry and no fallback request");
}

#[tokio::test]
async fn http_400_404_422_are_named_errors() {
    let cases = [
        (
            400_u16,
            "Bad Request",
            r#"{"message":"Either ref_audio or voice must be provided.","type":"invalid_request","code":"1000"}"#,
            vec![
                "mistral http 400",
                "type=invalid_request",
                "code=1000",
                "Either ref_audio or voice",
            ],
        ),
        (
            404,
            "Not Found",
            r#"{"message":"Voice not found","type":"invalid_voice","code":"1902"}"#,
            vec!["mistral http 404", "type=invalid_voice", "code=1902"],
        ),
        (
            422,
            "Unprocessable Entity",
            r#"{"message":{"detail":[{"loc":["body","input"],"msg":"Field required","type":"missing"},{"msg":"Input should be a valid string"}]}}"#,
            vec![
                "mistral http 422",
                "Field required",
                "Input should be a valid string",
            ],
        ),
    ];
    for (status, reason, body, needles) in cases {
        let mock = Mock::start(move |mut stream| async move {
            write_status(&mut stream, status, reason, body).await;
        })
        .await;
        let tts = provider(&mock.endpoint());
        let err = within(tts.synthesize("Secret customer sentence."))
            .await
            .expect_err("an error status is an error");
        let message = err.to_string();
        for needle in needles {
            assert!(
                message.contains(needle),
                "{status}: `{needle}` in {message}"
            );
        }
        assert!(!message.contains("Secret customer sentence."), "{message}");
        assert_eq!(mock.hits(), 1, "status {status} must not be retried");
    }
}

#[tokio::test]
async fn server_that_never_answers_times_out() {
    let mock = Mock::start(|stream| async move {
        // The request was read by the mock; answer nothing and keep the socket open.
        let _open = stream;
        std::future::pending::<()>().await;
    })
    .await;
    let tts = MistralTts::with_timeouts(
        &config(&mock.endpoint(), None, None),
        Timeouts {
            first_audio: Duration::from_millis(200),
            ..Timeouts::default()
        },
    )
    .unwrap();

    let started = std::time::Instant::now();
    let err = within(tts.synthesize("Never answered."))
        .await
        .expect_err("a silent server must time out");
    let message = err.to_string();
    assert!(
        message.contains("mistral http first audio timed out after 200 ms"),
        "{message}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!message.contains("Never answered."), "{message}");
    assert_eq!(mock.hits(), 1);
}

#[tokio::test]
async fn unknown_sse_event_is_skipped() {
    let pcm = samples(400);
    let mut wav = wav_header(1, 16, 24_000, 0, &[]);
    wav.extend_from_slice(&s16_bytes(&pcm));
    let mock = Mock::start(move |mut stream| {
        let wav = wav.clone();
        async move {
            write_sse_head(&mut stream).await;
            write_chunk(&mut stream, b": keepalive comment\n\n").await;
            write_event(
                &mut stream,
                &json!({"type": "speech.something.else", "x": 1}),
            )
            .await;
            write_delta(&mut stream, &wav).await;
            // An `event:` line is accepted next to the data line.
            write_chunk(
                &mut stream,
                b"event: speech.audio.done\ndata: {\"type\":\"speech.audio.done\"}\n\n",
            )
            .await;
            end_chunked(&mut stream).await;
        }
    })
    .await;
    let tts = provider(&mock.endpoint());
    let returned = within(tts.synthesize("Hello there.")).await.unwrap();
    assert_eq!(returned[0].pcm.to_vec(), expected_stereo(&pcm));
}

#[tokio::test]
async fn request_body_has_only_documented_fields() {
    let pcm = samples(100);
    let mut wav = wav_header(1, 16, 24_000, 0, &[]);
    wav.extend_from_slice(&s16_bytes(&pcm));
    let mock = Mock::start(move |stream| serve_deltas(stream, vec![wav.clone()])).await;
    // No model configured: the default is sent.
    let tts = MistralTts::new(&config(&mock.endpoint(), Some("gb_jane_neutral"), None)).unwrap();
    within(tts.synthesize("Hello there.")).await.unwrap();

    let req = mock.request(0);
    assert_eq!(req.request_line, "POST /v1/audio/speech HTTP/1.1");
    let mut keys: Vec<_> = req.body.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["input", "model", "response_format", "stream", "voice_id"]
    );
    assert_eq!(req.body["model"], "voxtral-mini-tts-3-0");
    assert_eq!(req.body["input"], "Hello there.");
    assert_eq!(req.body["voice_id"], "gb_jane_neutral");
    assert_eq!(req.body["response_format"], "wav");
    assert_eq!(req.body["stream"], true);
    assert_eq!(req.headers["authorization"], "Bearer test-key");
    assert_eq!(req.headers["content-type"], "application/json");
    assert_eq!(req.headers["accept"], "text/event-stream");
    assert_eq!(tts.last_transport().as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn every_documented_tts_model_is_accepted() {
    let pcm = samples(100);
    let mut wav = wav_header(1, 16, 24_000, 0, &[]);
    wav.extend_from_slice(&s16_bytes(&pcm));
    let mock = Mock::start(move |stream| serve_deltas(stream, vec![wav.clone()])).await;
    for (index, model) in [
        "voxtral-mini-tts-3-0",
        "voxtral-mini-tts-3",
        "voxtral-mini-tts-realtime-3-0",
    ]
    .into_iter()
    .enumerate()
    {
        let tts = MistralTts::new(&config(&mock.endpoint(), None, Some(model))).unwrap();
        within(tts.synthesize("Hello there.")).await.unwrap();
        assert_eq!(mock.request(index).body["model"], model, "sent verbatim");
    }
    assert_eq!(mock.hits(), 3);
}

#[tokio::test]
async fn default_voice_used_when_unset_or_default() {
    let pcm = samples(100);
    let mut wav = wav_header(1, 16, 24_000, 0, &[]);
    wav.extend_from_slice(&s16_bytes(&pcm));
    let mock = Mock::start(move |stream| serve_deltas(stream, vec![wav.clone()])).await;
    for voice in [None, Some(""), Some("default")] {
        let tts = MistralTts::new(&config(&mock.endpoint(), voice, None)).unwrap();
        within(tts.synthesize("Hello there.")).await.unwrap();
    }
    assert_eq!(mock.hits(), 3);
    for i in 0..3 {
        let body = mock.request(i).body;
        assert_eq!(body["voice_id"], "voice_478", "request {i}");
        assert_eq!(body["model"], "voxtral-mini-tts-3-0", "request {i}");
    }
}

#[tokio::test]
async fn unknown_model_is_rejected_before_any_request() {
    let mock = Mock::start(|mut stream| async move {
        write_status(&mut stream, 200, "OK", "{}").await;
    })
    .await;
    for model in ["voxtral-mini-tts-latest", "voxtral-mini-tts-2", "tts-1", ""] {
        let err = match MistralTts::new(&config(&mock.endpoint(), None, Some(model))) {
            Ok(_) => panic!("`{model}` must be rejected"),
            Err(err) => err,
        };
        assert!(matches!(err, SpeechError::Config(_)), "{err}");
        let message = err.to_string();
        assert!(
            message.contains(model) && message.contains("voxtral-mini-tts-3-0"),
            "{message}"
        );
    }
    assert_eq!(mock.hits(), 0, "nothing was sent");
}

#[tokio::test]
async fn missing_api_key_is_a_named_error() {
    let mock = Mock::start(|mut stream| async move {
        write_status(&mut stream, 200, "OK", "{}").await;
    })
    .await;
    std::env::remove_var("MISTRAL_API_KEY");
    let mut cfg = config(&mock.endpoint(), None, None);
    cfg.api_key = None;
    let tts = MistralTts::new(&cfg).unwrap();
    let err = within(tts.synthesize("Hello there.")).await.unwrap_err();
    assert!(err.to_string().contains("missing MISTRAL_API_KEY"), "{err}");
    assert_eq!(mock.hits(), 0);
}

// ---- MISTRAL_BASE_URL ---------------------------------------------------------------------

/// The env is process-global: tests that set it hold this lock and restore it. Every other test
/// in this file passes an explicit endpoint, which wins over the env.
static BASE_URL_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn wav_ok() -> Vec<u8> {
    let mut wav = wav_header(1, 16, 24_000, 0, &[]);
    wav.extend_from_slice(&s16_bytes(&samples(100)));
    wav
}

#[tokio::test]
async fn env_base_url_is_used_when_the_config_has_none() {
    let _guard = BASE_URL_ENV_LOCK.lock().await;
    let wav = wav_ok();
    let mock = Mock::start(move |stream| serve_deltas(stream, vec![wav.clone()])).await;
    std::env::set_var("MISTRAL_BASE_URL", mock.endpoint());
    let mut cfg = config("unused", None, None);
    cfg.endpoint = None;
    let tts = MistralTts::new(&cfg).unwrap();
    std::env::remove_var("MISTRAL_BASE_URL");
    within(tts.synthesize("Hello there.")).await.unwrap();
    assert_eq!(mock.hits(), 1);
    assert_eq!(
        mock.request(0).request_line,
        "POST /v1/audio/speech HTTP/1.1"
    );
}

#[tokio::test]
async fn config_base_url_wins_over_env() {
    let _guard = BASE_URL_ENV_LOCK.lock().await;
    let wav = wav_ok();
    let from_config = Mock::start({
        let wav = wav.clone();
        move |stream| serve_deltas(stream, vec![wav.clone()])
    })
    .await;
    let from_env = Mock::start(move |stream| serve_deltas(stream, vec![wav.clone()])).await;
    std::env::set_var("MISTRAL_BASE_URL", from_env.endpoint());
    let tts = MistralTts::new(&config(&from_config.endpoint(), None, None)).unwrap();
    std::env::remove_var("MISTRAL_BASE_URL");
    within(tts.synthesize("Hello there.")).await.unwrap();
    assert_eq!(from_config.hits(), 1);
    assert_eq!(from_env.hits(), 0, "the env endpoint must not be called");
}
