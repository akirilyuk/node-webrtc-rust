//! Mock-server tests for the Deepgram live STT provider. Run with `--features live`.
//!
//! Guards the utterance contract: Deepgram sends several `is_final` segments per spoken phrase
//! (and `speech_final` at its own endpoint), and the provider turns them into exactly one
//! `Final` per utterance, at the agent's finalize (`rt.late-final-next-utterance`,
//! `rt.stale-event-by-key`). The mock plays the server by hand and only answers after it read
//! the client frame it answers, so ordering comes from the protocol, not from sleeps.
#![cfg(feature = "live")]

use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SttOutput, SttProvider, SttTranscript, VendorFactory,
};
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, WebSocketStream};

type Ws = WebSocketStream<TcpStream>;

/// 20 ms of 16 kHz mono s16le.
const FRAME: usize = 640;

async fn within<T>(fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("step did not finish within 10 s")
}

async fn start() -> (Box<dyn SttProvider>, Ws) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let accept = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        accept_async(stream).await.unwrap()
    });
    let mut stt = DeepgramFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Deepgram,
            model: None,
            model_path: None,
            language: None,
            api_key: Some("test-key".into()),
            endpoint: Some(endpoint),
            transport: None,
        })
        .unwrap();
    within(stt.start()).await.unwrap();
    (stt, within(accept).await.unwrap())
}

async fn push_ms(stt: &mut dyn SttProvider, ms: usize) {
    for _ in 0..ms / 20 {
        stt.push_audio(Bytes::from(vec![1u8; FRAME])).await.unwrap();
    }
}

/// Reads client frames until the `Finalize` text message (all audio before it has arrived).
async fn read_until_finalize(ws: &mut Ws) -> usize {
    let mut audio = 0;
    loop {
        match within(ws.next()).await.expect("socket closed").unwrap() {
            Message::Binary(b) => audio += b.len(),
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                assert_eq!(v["type"], "Finalize");
                return audio;
            }
            _ => {}
        }
    }
}

async fn send_results(
    ws: &mut Ws,
    text: &str,
    is_final: bool,
    speech_final: bool,
    start: f64,
    dur: f64,
    from_finalize: bool,
) {
    let msg = json!({
        "type": "Results",
        "start": start,
        "duration": dur,
        "is_final": is_final,
        "speech_final": speech_final,
        "from_finalize": from_finalize,
        "channel": {"alternatives": [{"transcript": text}]},
    });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
}

/// Polls until `done(outputs)` or the bound; returns every output seen.
async fn collect(stt: &mut dyn SttProvider, done: impl Fn(&[SttOutput]) -> bool) -> Vec<SttOutput> {
    let mut seen = Vec::new();
    within(async {
        while !done(&seen) {
            match stt.poll_output().await.unwrap() {
                Some(o) => seen.push(o),
                None => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await;
    seen
}

fn finals(outputs: &[SttOutput]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|o| match o {
            SttOutput::Transcript(SttTranscript::Final(t)) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn has_done(outputs: &[SttOutput]) -> bool {
    outputs.iter().any(|o| matches!(o, SttOutput::FinalizeDone))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn is_final_segments_of_one_utterance_are_one_final_at_the_finalize() {
    let (mut stt, mut ws) = start().await;
    assert_eq!(stt.finalize_mode(), FinalizeMode::Deferred);
    push_ms(stt.as_mut(), 5000).await;
    // Deepgram's own endpoint fired mid-utterance: two is_final segments, the second also
    // speech_final, then an interim for the tail.
    send_results(&mut ws, "one two three", true, false, 0.0, 2.0, false).await;
    send_results(&mut ws, "four five six", true, true, 2.0, 1.0, false).await;
    send_results(&mut ws, "seven", false, false, 3.0, 0.5, false).await;
    // Nothing is final before the agent says the utterance ended.
    let before = collect(stt.as_mut(), |seen| seen.len() >= 3).await;
    assert!(
        finals(&before).is_empty(),
        "a segment leaked out as a final"
    );

    stt.request_finalize().await.unwrap();
    read_until_finalize(&mut ws).await;
    send_results(&mut ws, "seven eight nine ten", true, true, 3.0, 2.0, true).await;
    let after = collect(stt.as_mut(), has_done).await;
    assert_eq!(
        finals(&after),
        ["one two three four five six seven eight nine ten"]
    );
    // The final comes before the marker, and nothing else follows it.
    assert!(matches!(after.last(), Some(SttOutput::FinalizeDone)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_answer_without_text_still_closes_with_what_was_received() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 2000).await;
    send_results(&mut ws, "hello there", true, true, 0.0, 1.5, false).await;
    collect(stt.as_mut(), |seen| !seen.is_empty()).await;
    stt.request_finalize().await.unwrap();
    read_until_finalize(&mut ws).await;
    send_results(&mut ws, "", true, false, 1.5, 0.0, true).await;
    let out = collect(stt.as_mut(), has_done).await;
    assert_eq!(finals(&out), ["hello there"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segment_arriving_after_the_finalize_never_joins_the_next_utterance() {
    let (mut stt, mut ws) = start().await;
    // Utterance 1: 1 s of audio.
    push_ms(stt.as_mut(), 1000).await;
    send_results(&mut ws, "first utterance", true, false, 0.0, 0.8, false).await;
    collect(stt.as_mut(), |seen| !seen.is_empty()).await;
    stt.request_finalize().await.unwrap();
    read_until_finalize(&mut ws).await;
    send_results(&mut ws, "", true, false, 0.8, 0.0, true).await;
    let first = collect(stt.as_mut(), has_done).await;
    assert_eq!(finals(&first), ["first utterance"]);

    // A late segment of utterance 1 (its audio ended before the finalize boundary), then
    // utterance 2.
    send_results(&mut ws, "stale tail", true, false, 0.4, 0.5, false).await;
    push_ms(stt.as_mut(), 1000).await;
    send_results(&mut ws, "second utterance", true, false, 1.0, 0.8, false).await;
    collect(stt.as_mut(), |seen| !seen.is_empty()).await;
    stt.request_finalize().await.unwrap();
    read_until_finalize(&mut ws).await;
    send_results(&mut ws, "", true, false, 1.8, 0.0, true).await;
    let second = collect(stt.as_mut(), has_done).await;
    assert_eq!(finals(&second), ["second utterance"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_stream_finalize_is_answered_without_waiting_for_the_vendor() {
    let (mut stt, _ws) = start().await;
    stt.request_finalize().await.unwrap();
    let out = collect(stt.as_mut(), has_done).await;
    assert!(finals(&out).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_closed_with_a_pending_finalize_answers_it_with_an_error() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 1000).await;
    send_results(&mut ws, "partial words", true, false, 0.0, 0.8, false).await;
    collect(stt.as_mut(), |seen| !seen.is_empty()).await;
    stt.request_finalize().await.unwrap();
    read_until_finalize(&mut ws).await;
    drop(ws);
    let mut seen = Vec::new();
    let mut saw_error = false;
    within(async {
        loop {
            match stt.poll_output().await {
                Ok(Some(o)) => {
                    let done = matches!(o, SttOutput::FinalizeDone);
                    seen.push(o);
                    if done {
                        break;
                    }
                }
                Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(_) => saw_error = true,
            }
        }
    })
    .await;
    assert!(saw_error);
    assert_eq!(finals(&seen), ["partial words"]);
}
