//! The real `VoiceAgent` and STT actor (production VAD preset) against a mock Deepgram server
//! that splits one spoken phrase into several `is_final` segments, as staging did
//! (`rt.late-final-next-utterance`): "one two three four five six" and "seven eight nine ten"
//! arrived as two `user_speech_final` events. The agent must report one final with every
//! segment, in order. Runs in real time (about 12 s): the agent paces by wall clock.
#![cfg(feature = "live")]

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;
use node_webrtc_rust_vendor_live_support::audio::COUNT_WORDS;
use node_webrtc_rust_vendor_live_support::fullstack::run_fullstack_settling;
use node_webrtc_rust_vendor_live_support::vad_gate::tone_stereo_48k;
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

/// 16 kHz mono s16le.
const BYTES_PER_S: usize = 32_000;

fn results(
    text: &str,
    is_final: bool,
    speech_final: bool,
    start: f64,
    dur: f64,
    from_finalize: bool,
) -> Message {
    Message::Text(
        json!({
            "type": "Results",
            "start": start,
            "duration": dur,
            "is_final": is_final,
            "speech_final": speech_final,
            "from_finalize": from_finalize,
            "channel": {"alternatives": [{"transcript": text}]},
        })
        .to_string()
        .into(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_phrase_split_into_is_final_segments_is_one_user_speech_final() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let mut bytes = 0usize;
        let mut sent = 0;
        while let Some(Ok(message)) = ws.next().await {
            match message {
                Message::Binary(chunk) => {
                    bytes += chunk.len();
                    // Deepgram's endpointing fires twice while the speaker is still counting.
                    if sent == 0 && bytes >= 2 * BYTES_PER_S {
                        sent = 1;
                        ws.send(results("one two three", true, false, 0.0, 1.8, false))
                            .await
                            .unwrap();
                    } else if sent == 1 && bytes >= 3 * BYTES_PER_S {
                        sent = 2;
                        ws.send(results("four five six", true, true, 1.8, 1.0, false))
                            .await
                            .unwrap();
                    }
                }
                Message::Text(text) => {
                    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if v["type"] == "Finalize" {
                        let end = bytes as f64 / BYTES_PER_S as f64;
                        ws.send(results(
                            "seven eight nine ten",
                            true,
                            true,
                            2.8,
                            end - 2.8,
                            true,
                        ))
                        .await
                        .unwrap();
                    }
                }
                _ => {}
            }
        }
    });

    let speech = tone_stereo_48k(3600, -6.0);
    let sample = run_fullstack_settling(
        Arc::new(DeepgramFactory) as Arc<dyn VendorFactory>,
        SttConfig {
            provider: SttVendor::Deepgram,
            model: None,
            model_path: None,
            language: None,
            api_key: Some("test-key".into()),
            endpoint: Some(endpoint),
            transport: None,
        },
        &speech,
        &COUNT_WORDS,
        Duration::from_millis(1500),
    )
    .await
    .unwrap();
    server.abort();
    assert_eq!(sample.not_found, 0, "{sample:?}");
    assert_eq!(sample.finals, 1, "{sample:?}");
    assert_eq!(sample.matched_words, 10, "{sample:?}");
}
