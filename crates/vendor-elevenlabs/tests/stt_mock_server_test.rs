//! Mock-server tests for the ElevenLabs Scribe realtime STT provider. Run with `--features live`.
//!
//! Guards the utterance contract: `commit` is answered by a `committed_transcript` some time
//! later, and the provider ends the utterance (`FinalizeDone`) only when that answer arrived, so
//! the voice agent never closes it as `user_stt_not_found` first
//! (`rt.stale-event-by-key`, `rt.late-final-next-utterance`). The mock plays the server by hand
//! and only answers after it read the client frame it answers.
#![cfg(feature = "live")]

use std::future::Future;
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, SttOutput, SttProvider, SttTranscript, VendorFactory,
};
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, WebSocketStream};

type Ws = WebSocketStream<TcpStream>;

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
    let mut stt = ElevenLabsFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Elevenlabs,
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

/// Reads client frames until a commit; returns the decoded audio bytes before it.
async fn read_until_commit(ws: &mut Ws) -> usize {
    let mut audio = 0;
    loop {
        let Message::Text(t) = within(ws.next()).await.expect("socket closed").unwrap() else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["message_type"], "input_audio_chunk");
        if v["commit"] == true {
            assert_eq!(v["audio_base_64"], "");
            return audio;
        }
        audio += base64::engine::general_purpose::STANDARD
            .decode(v["audio_base_64"].as_str().unwrap())
            .unwrap()
            .len();
    }
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

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
async fn finalize_waits_for_the_committed_transcript() {
    let (mut stt, mut ws) = start().await;
    assert_eq!(stt.finalize_mode(), FinalizeMode::Deferred);
    push_ms(stt.as_mut(), 5000).await;
    send(
        &mut ws,
        json!({"message_type": "partial_transcript", "text": "one two"}),
    )
    .await;
    let before = collect(stt.as_mut(), |seen| !seen.is_empty()).await;
    assert!(finals(&before).is_empty());

    stt.request_finalize().await.unwrap();
    // The commit reached the server after all 5 s of audio.
    assert_eq!(read_until_commit(&mut ws).await, 5000 / 20 * FRAME);
    // No marker before the server answered: the agent keeps the utterance open.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(stt.poll_output().await.unwrap().is_none());

    send(
        &mut ws,
        json!({"message_type": "committed_transcript", "text": "one two three four five"}),
    )
    .await;
    let out = collect(stt.as_mut(), has_done).await;
    assert_eq!(finals(&out), ["one two three four five"]);
    assert!(matches!(out.last(), Some(SttOutput::FinalizeDone)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_auto_commit_before_the_finalize_joins_the_utterance() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 2000).await;
    send(
        &mut ws,
        json!({"message_type": "committed_transcript", "text": "first half"}),
    )
    .await;
    let early = collect(stt.as_mut(), |seen| !seen.is_empty()).await;
    assert!(
        finals(&early).is_empty(),
        "an auto commit must not be a final"
    );
    stt.request_finalize().await.unwrap();
    read_until_commit(&mut ws).await;
    send(
        &mut ws,
        json!({"message_type": "committed_transcript", "text": "second half"}),
    )
    .await;
    let out = collect(stt.as_mut(), has_done).await;
    assert_eq!(finals(&out), ["first half second half"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_committed_transcript_ends_the_utterance_without_a_final() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 1000).await;
    stt.request_finalize().await.unwrap();
    read_until_commit(&mut ws).await;
    send(
        &mut ws,
        json!({"message_type": "committed_transcript", "text": ""}),
    )
    .await;
    let out = collect(stt.as_mut(), has_done).await;
    assert!(finals(&out).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_pair_with_their_commits_in_order() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 1000).await;
    stt.request_finalize().await.unwrap();
    read_until_commit(&mut ws).await;
    push_ms(stt.as_mut(), 1000).await;
    stt.request_finalize().await.unwrap();
    read_until_commit(&mut ws).await;
    send(
        &mut ws,
        json!({"message_type": "committed_transcript", "text": "alpha"}),
    )
    .await;
    send(
        &mut ws,
        json!({"message_type": "committed_transcript", "text": "beta"}),
    )
    .await;
    let mut out = Vec::new();
    while out
        .iter()
        .filter(|o| matches!(o, SttOutput::FinalizeDone))
        .count()
        < 2
    {
        out.extend(collect(stt.as_mut(), has_done).await);
    }
    let order: Vec<String> = out
        .iter()
        .filter_map(|o| match o {
            SttOutput::Transcript(SttTranscript::Final(t)) => Some(t.clone()),
            SttOutput::FinalizeDone => Some("|".into()),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["alpha", "|", "beta", "|"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_with_nothing_pushed_is_answered_at_once() {
    let (mut stt, _ws) = start().await;
    stt.request_finalize().await.unwrap();
    let out = collect(stt.as_mut(), has_done).await;
    assert!(finals(&out).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_throttled_answers_the_commit_and_other_errors_do_not() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 1000).await;
    stt.request_finalize().await.unwrap();
    read_until_commit(&mut ws).await;
    send(
        &mut ws,
        json!({"message_type": "rate_limited", "error": "slow"}),
    )
    .await;
    // An error that is not about the commit leaves it pending.
    let mut saw_error = false;
    within(async {
        while !saw_error {
            if stt.poll_output().await.is_err() {
                saw_error = true;
            } else {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    })
    .await;
    assert!(stt.poll_output().await.unwrap().is_none());
    send(
        &mut ws,
        json!({"message_type": "commit_throttled", "error": "too fast"}),
    )
    .await;
    let mut result: SpeechResult<Option<SttOutput>> = Ok(None);
    within(async {
        loop {
            result = stt.poll_output().await;
            match &result {
                Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                _ => break,
            }
        }
    })
    .await;
    assert!(result.is_err(), "the throttle error is reported");
    let out = collect(stt.as_mut(), has_done).await;
    assert!(finals(&out).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_closed_with_a_pending_commit_answers_it_with_an_error() {
    let (mut stt, mut ws) = start().await;
    push_ms(stt.as_mut(), 1000).await;
    stt.request_finalize().await.unwrap();
    read_until_commit(&mut ws).await;
    drop(ws);
    let mut saw_error = false;
    within(async {
        loop {
            match stt.poll_output().await {
                Ok(Some(SttOutput::FinalizeDone)) => break,
                Ok(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(_) => saw_error = true,
            }
        }
    })
    .await;
    assert!(saw_error);
}
