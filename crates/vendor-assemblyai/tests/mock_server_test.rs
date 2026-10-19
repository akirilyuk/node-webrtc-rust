//! Local mock AssemblyAI v3 server. Guards the session behaviour: partials and finals from `Turn`
//! messages, a Ping that must not end the session, the documented `ForceEndpoint` finalize that
//! keeps the session for the next utterance, `Terminate` on stop, and a closed socket reported by
//! the next push.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const BOUND: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Seen {
    uris: Vec<String>,
    binary: usize,
    texts: Vec<String>,
}

struct Mock {
    endpoint: String,
    accepts: Arc<AtomicUsize>,
    seen: Arc<Mutex<Seen>>,
}

/// `behave`: runs per accepted connection with the socket and the shared record.
#[allow(clippy::result_large_err)] // the handshake callback signature is fixed by tungstenite
async fn start<F, Fut>(behave: F) -> Mock
where
    F: Fn(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Arc<Mutex<Seen>>) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let accepts = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let behave = Arc::new(behave);
    {
        let accepts = accepts.clone();
        let seen = seen.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                accepts.fetch_add(1, Ordering::SeqCst);
                let seen = seen.clone();
                let behave = behave.clone();
                tokio::spawn(async move {
                    let uris = seen.clone();
                    let ws = tokio_tungstenite::accept_hdr_async(stream, move |req: &_, resp| {
                        let req: &tokio_tungstenite::tungstenite::handshake::server::Request = req;
                        uris.lock().unwrap().uris.push(req.uri().to_string());
                        Ok(resp)
                    })
                    .await
                    .unwrap();
                    behave(ws, seen).await;
                });
            }
        });
    }
    Mock {
        endpoint,
        accepts,
        seen,
    }
}

fn stt(endpoint: &str) -> Box<dyn SttProvider> {
    AssemblyAiFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Assemblyai,
            model: None,
            model_path: None,
            language: None,
            api_key: Some("k".into()),
            endpoint: Some(endpoint.to_string()),
            transport: None,
        })
        .unwrap()
}

fn turn(text: &str, end_of_turn: bool) -> Message {
    Message::Text(
        serde_json::json!({"type": "Turn", "transcript": text, "end_of_turn": end_of_turn})
            .to_string()
            .into(),
    )
}

async fn next_transcript(provider: &mut dyn SttProvider) -> SttTranscript {
    tokio::time::timeout(BOUND, async {
        loop {
            if let Some(t) = provider.poll_transcript().await.unwrap() {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no transcript in time")
}

/// Echoes a partial for the first audio frame of a turn and a final on `ForceEndpoint`.
async fn turn_server(
    mut ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    seen: Arc<Mutex<Seen>>,
) {
    ws.send(Message::Ping(Bytes::from_static(b"p")))
        .await
        .unwrap();
    let mut audio_in_turn = false;
    while let Some(Ok(msg)) = ws.next().await {
        match msg {
            Message::Binary(_) => {
                seen.lock().unwrap().binary += 1;
                if !audio_in_turn {
                    audio_in_turn = true;
                    ws.send(turn("hello", false)).await.unwrap();
                }
            }
            Message::Text(text) => {
                seen.lock().unwrap().texts.push(text.to_string());
                if text.contains("ForceEndpoint") {
                    audio_in_turn = false;
                    ws.send(turn("hello world", true)).await.unwrap();
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

#[tokio::test]
async fn partials_finals_and_two_utterances_on_one_session() {
    let mock = start(turn_server).await;
    let mut provider = stt(&mock.endpoint);
    provider.start().await.unwrap();
    assert_eq!(provider.last_transport().as_deref(), Some("ws"));

    for _ in 0..2 {
        provider
            .push_audio(Bytes::from_static(&[1u8; 640]))
            .await
            .unwrap();
        assert_eq!(
            next_transcript(provider.as_mut()).await,
            SttTranscript::Partial("hello".into()),
            "a Ping before the turn must not end the session"
        );
        provider.finalize_utterance().await.unwrap();
        assert_eq!(
            next_transcript(provider.as_mut()).await,
            SttTranscript::Final("hello world".into())
        );
    }
    provider.stop().await.unwrap();

    assert_eq!(
        mock.accepts.load(Ordering::SeqCst),
        1,
        "one socket for two utterances"
    );
    // `stop` sends the documented Terminate before the socket goes away.
    tokio::time::timeout(BOUND, async {
        loop {
            if mock
                .seen
                .lock()
                .unwrap()
                .texts
                .iter()
                .any(|t| t.contains("Terminate"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no Terminate seen");
    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.binary, 2);
    assert_eq!(
        seen.texts
            .iter()
            .filter(|t| t.contains("ForceEndpoint"))
            .count(),
        2
    );
    assert!(seen.uris[0].starts_with("/v3/ws?"), "{}", seen.uris[0]);
    assert!(seen.uris[0].contains("speech_model=universal-streaming-english"));
    assert!(seen.uris[0].contains("encoding=pcm_s16le"));
}

#[tokio::test]
async fn push_after_the_server_closed_reports_a_named_error() {
    let mock = start(|mut ws, _seen| async move {
        let _ = ws.close(None).await;
    })
    .await;
    let mut provider = stt(&mock.endpoint);
    provider.start().await.unwrap();
    let outcome = tokio::time::timeout(BOUND, async {
        loop {
            match provider.push_audio(Bytes::from_static(&[1u8; 640])).await {
                Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(err) => return err,
            }
        }
    })
    .await
    .expect("the closed socket was never reported");
    match outcome {
        SpeechError::Vendor { vendor, message } => {
            assert_eq!(vendor, "assemblyai");
            assert_eq!(message, "assemblyai stt websocket closed");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn stop_returns_even_when_the_server_never_answers_the_close() {
    let mock = start(|ws, _seen| async move {
        let _ws = ws;
        std::future::pending::<()>().await;
    })
    .await;
    let mut provider = stt(&mock.endpoint);
    provider.start().await.unwrap();
    provider
        .push_audio(Bytes::from_static(&[1u8; 640]))
        .await
        .unwrap();
    tokio::time::timeout(BOUND, provider.stop())
        .await
        .expect("stop hung")
        .unwrap();
}
