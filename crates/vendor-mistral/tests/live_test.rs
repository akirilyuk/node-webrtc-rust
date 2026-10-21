//! Live Voxtral 3 checks against the real Mistral API. Run with `--features live`.
//!
//! Needs `MISTRAL_API_KEY` (CI maps the repo secret of the same name). Without a key the tests
//! print a note and return, unless `MISTRAL_LIVE_REQUIRED=1`, which turns a missing key into a
//! panic so CI can never go green on a skipped run. The key is never printed. Optional
//! `MISTRAL_BASE_URL` picks the endpoint (global, EU or US); every test honours it.
//!
//! The STT test synthesizes the counting phrase with Mistral TTS, gates it with the voice
//! agent's default VAD so only speech is streamed (realtime STT is billed per streamed minute),
//! paces it in real time and never prints the transcript.
//!
//! Run with `--test-threads=1`; the tests print timing lines (`mistral_*`) that CI copies into
//! the step summary.
#![cfg(feature = "live")]

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use node_webrtc_rust_vendor_live_support::audio::{COUNT_PHRASE, COUNT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt::measure_stt;
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, STT_CLIP_TAIL_MS};
use node_webrtc_rust_vendor_mistral::{resolve_base_url_from_env, MistralStt, MistralTts};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const TEXT: &str = "Hello from Voxtral three. This is a streaming check.";
/// 48 kHz stereo s16le: 4 bytes per frame, 192 bytes per millisecond.
const BYTES_PER_MS: usize = 192;

fn live_key() -> Option<String> {
    // Vendor id `mistral`: a missing key panics under `MISTRAL_LIVE_REQUIRED=1`, otherwise skips.
    live_env("mistral", &["MISTRAL_API_KEY"])
        .map(|mut env| env.remove("MISTRAL_API_KEY").expect("key"))
}

fn tts(key: &str, model: &str) -> MistralTts {
    // `endpoint: None`: the crate takes MISTRAL_BASE_URL, else the global endpoint.
    MistralTts::new(&TtsConfig {
        provider: TtsVendor::Mistral,
        model: Some(model.to_string()),
        model_path: None,
        voice: None,
        api_key: Some(key.to_string()),
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap_or_else(|err| panic!("tts config: {err}"))
}

struct SinkStats {
    chunks: usize,
    bytes: usize,
    first_at: Option<Instant>,
}

/// Sink plus a task that timestamps the first chunk and counts what arrives.
fn sink() -> (TtsProgressiveSink, JoinHandle<SinkStats>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<TtsAudioChunk>();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let task = tokio::spawn(async move {
        let mut stats = SinkStats {
            chunks: 0,
            bytes: 0,
            first_at: None,
        };
        while let Some(chunk) = rx.recv().await {
            stats.first_at.get_or_insert_with(Instant::now);
            stats.chunks += 1;
            stats.bytes += chunk.pcm.len();
        }
        stats
    });
    (sink, task)
}

async fn assert_streams(model: &str) {
    let Some(key) = live_key() else { return };
    println!("mistral_live_endpoint {}", resolve_base_url_from_env(None));
    let provider = tts(&key, model);
    let (sink, task) = sink();

    let started = Instant::now();
    let result = provider.synthesize_progressive(TEXT, Some(sink)).await;
    // The provider dropped its sink clone on return; the task ends once the channel closes.
    let chunks = match result {
        Ok(chunks) => chunks,
        Err(err) => panic!("mistral tts {model} failed: {err}"),
    };
    let stats = task.await.expect("sink task");

    let first_at = stats.first_at.expect("no sink chunk arrived");
    let first_audio_ms = first_at.duration_since(started).as_millis();
    println!("mistral_tts_first_audio_ms model={model} {first_audio_ms}");

    let returned: usize = chunks.iter().map(|c| c.pcm.len()).sum();
    let total_ms = returned / BYTES_PER_MS;
    println!(
        "mistral_tts_audio model={model} duration_ms={total_ms} sink_chunks={}",
        stats.chunks
    );
    assert!(
        total_ms > 500,
        "decoded audio is {total_ms} ms, expected > 500"
    );
    assert_eq!(
        stats.bytes, returned,
        "sink bytes differ from returned bytes"
    );
}

/// Documented `GET {base}/v1/models`, shape `{"data": [{"id": ..}, ..]}`. Bounded to 10 s.
async fn listed_model_ids(base: &str, key: &str) -> Vec<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");
    let response = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(key)
        .send()
        .await
        .unwrap_or_else(|err| panic!("GET /v1/models failed: {}", err.without_url()));
    assert!(
        response.status().is_success(),
        "GET /v1/models answered {}",
        response.status()
    );
    let body: serde_json::Value = response
        .json()
        .await
        .unwrap_or_else(|err| panic!("unreadable /v1/models body: {}", err.without_url()));
    body.get("data")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The realtime TTS model is Premier-tagged. When the key's model list has it, it must stream;
/// when it does not, synthesis must fail with `400 invalid_model`. The second branch starts
/// failing once Mistral enables the model for our key, which is the signal to revisit the default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_realtime_3_access_matches_the_model_list() {
    const REALTIME: &str = "voxtral-mini-tts-realtime-3-0";
    let Some(key) = live_key() else { return };
    let base = resolve_base_url_from_env(None);
    let listed = listed_model_ids(&base, &key)
        .await
        .iter()
        .any(|id| id == REALTIME);
    println!("mistral_tts_realtime_access {{\"listed\":{listed}}}");
    if listed {
        assert_streams(REALTIME).await;
        return;
    }
    let (sink, task) = sink();
    let err = tts(&key, REALTIME)
        .synthesize_progressive(TEXT, Some(sink))
        .await
        .expect_err("the realtime model is not listed, so synthesis must fail");
    let _ = task.await;
    let message = err.to_string();
    assert!(
        message.contains("400") && message.contains("invalid_model"),
        "unexpected error: {message}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_3_streams() {
    assert_streams("voxtral-mini-tts-3-0").await;
}

/// Voxtral TTS speaks the counting phrase; the VAD gate drops the silence padding; the gated
/// 16 kHz frames stream to the realtime model at real-time pace; one finalize ends the
/// utterance. Only counts and timings are printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_realtime_3_live_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };

    let speech: Vec<u8> = tts(&key, "voxtral-mini-tts-3-0")
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("voxtral tts failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    // 1 s of silence before and a tail longer than the VAD hangover after, so the speech
    // segment ends.
    let stereo = [
        silence_stereo_48k(1000),
        speech,
        silence_stereo_48k(STT_CLIP_TAIL_MS),
    ]
    .concat();

    let mut stt = MistralStt::new(&SttConfig {
        provider: SttVendor::Mistral,
        model: None,
        model_path: None,
        language: Some("en".into()),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    })
    .unwrap_or_else(|err| panic!("stt config: {err}"));

    let sample = measure_stt(&mut stt, &stereo, &COUNT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("stt roundtrip failed: {err}"));
    use node_webrtc_rust_speech::pipeline::SttProvider;
    let _ = stt.stop().await;

    println!("mistral_stt_open_ms {}", sample.open_ms);
    println!(
        "mistral_stt_commit_to_final_ms {}",
        sample.finalize_to_final_ms
    );
    println!(
        "mistral_stt_speech_end_to_final_ms {}",
        sample.speech_end_to_final_ms
    );
    println!(
        "mistral_stt_pushed_ms {}/{}",
        sample.pushed_ms, sample.total_ms
    );
    println!(
        "mistral_stt_words {}/{} partials={}",
        sample.matched_words,
        COUNT_WORDS.len(),
        sample.partials_before_finalize
    );
    assert!(
        sample.matched_words >= 7,
        "matched {} of {} counting words (transcript not printed)",
        sample.matched_words,
        COUNT_WORDS.len()
    );
    assert!(
        sample.pushed_ms < sample.total_ms.saturating_sub(1000),
        "the silence padding was streamed: {} of {} ms",
        sample.pushed_ms,
        sample.total_ms
    );
}
