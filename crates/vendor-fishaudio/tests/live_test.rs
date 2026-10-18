//! Live Drama 3 checks against the real Fish Audio API. Run with `--features live`.
//!
//! Needs `FISH_API_KEY` (CI maps the repo secret `FISH_AUDIO_API` to it). Without a key the
//! tests print a note and return, unless `FISH_LIVE_REQUIRED=1`, which turns a missing key
//! into a panic so CI can never go green on a skipped run. The key is never printed.
//! Optional `FISH_VOICE_ID` picks a `reference_id` for the synthesis tests.
//!
//! The STT test synthesizes a known phrase with Drama 3, gates it with the voice agent's
//! default VAD so only speech is streamed to Fish (STT is billed for the audio streamed), and
//! never prints the transcript.
//!
//! Run with `--test-threads=1`: the Starter tier allows 5 concurrent requests and the tests
//! print timing lines (`fishaudio_*`) that CI copies into the step summary.
#![cfg(feature = "live")]

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::{
    SttOutput, SttProvider, SttTranscript, TtsAudioChunk, TtsProgressiveSink,
};
use node_webrtc_rust_vendor_fishaudio::__test::Client;
use node_webrtc_rust_vendor_fishaudio::FishAudioStt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use node_webrtc_rust_vendor_live_support::audio::{matched_words, rms, STT_PHRASE, STT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, vad_gated_frames};

const MODEL: &str = "drama-3";
const TEXT: &str =
    "[warm, friendly] Hello from Drama three. <emphasis>This</emphasis> is a streaming check.";
/// 48 kHz stereo s16le: 4 bytes per frame, 192 bytes per millisecond.
const BYTES_PER_MS: usize = 192;

fn live_key() -> Option<String> {
    // Vendor id `fish`: a missing key panics under `FISH_LIVE_REQUIRED=1`, otherwise skips.
    live_env("fish", &["FISH_API_KEY"]).map(|mut env| env.remove("FISH_API_KEY").expect("key"))
}

fn env_voice() -> Option<String> {
    std::env::var("FISH_VOICE_ID")
        .ok()
        .filter(|voice| !voice.is_empty())
}

fn client(key: String) -> Client {
    Client::new(Some(key), None).unwrap_or_else(|err| panic!("client: {err}"))
}

fn duration_ms(pcm_len: usize) -> usize {
    pcm_len / BYTES_PER_MS
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

/// Runs one transport with a sink attached and checks the streaming contract.
async fn assert_streams(transport: &str) {
    let Some(key) = live_key() else { return };
    let client = client(key);
    let voice = env_voice();
    let (sink, task) = sink();

    let started = Instant::now();
    let result = if transport == "ws" {
        client
            .synthesize_ws(TEXT, voice.as_deref(), MODEL, Some(&sink))
            .await
    } else {
        client
            .synthesize_http(TEXT, voice.as_deref(), MODEL, Some(&sink))
            .await
    };
    let returned_at = Instant::now();
    // Dropping the sink closes the channel so the receiver task finishes.
    drop(sink);
    let pcm = match result {
        Ok(pcm) => pcm,
        Err(err) => panic!("fishaudio {transport} synthesis failed: {err}"),
    };
    let stats = task.await.expect("sink task");

    let first_at = stats.first_at.expect("no sink chunk arrived");
    let ttfa_ms = first_at.duration_since(started).as_millis();
    println!("fishaudio_ttfa_ms transport={transport} {ttfa_ms}");

    assert!(
        stats.chunks >= 2,
        "expected >= 2 sink chunks, got {}",
        stats.chunks
    );
    assert!(
        first_at < returned_at,
        "first sink chunk arrived after the call returned"
    );
    let total_ms = duration_ms(pcm.len());
    assert!(
        (1500..=15000).contains(&total_ms),
        "audio duration {total_ms} ms outside 1500..=15000"
    );
    assert_eq!(
        stats.bytes,
        pcm.len(),
        "sink bytes differ from returned bytes"
    );
    let level = rms(&pcm);
    assert!(level > 200.0, "RMS {level:.1} <= 200: audio is near-silent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drama3_live_ws_streams() {
    assert_streams("ws").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drama3_live_http_streams() {
    assert_streams("http").await;
}

/// Decides whether Fish accepts a request without `reference_id` (plan §9 Q3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drama3_live_without_voice() {
    let Some(key) = live_key() else { return };
    let client = client(key);
    match client.synthesize_ws(TEXT, None, MODEL, None).await {
        Ok(pcm) => {
            let ms = duration_ms(pcm.len());
            println!("fishaudio_no_voice status=ok duration_ms={ms}");
            assert!(ms > 1000, "no-voice audio too short: {ms} ms");
        }
        Err(err) => {
            println!("fishaudio_no_voice status=error message={err}");
            panic!("synthesis without reference_id failed: {err}");
        }
    }
}

/// Drama 3 speaks a known phrase; the VAD gate drops the silence padding; the gated 16 kHz
/// frames stream to `transcribe-1` at real-time pace; one commit ends the utterance.
/// The transcript is never printed, only the matched word count and timings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_1_live_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };

    // 48 kHz stereo s16le of the phrase, 1 s of silence before and 2 s after. The tail is longer
    // than the VAD's default 1300 ms hangover, otherwise the speech segment never ends.
    let voice = env_voice();
    let speech = client(key.clone())
        .synthesize_ws(STT_PHRASE, voice.as_deref(), MODEL, None)
        .await
        .unwrap_or_else(|err| panic!("drama 3 synthesis failed: {err}"));
    let stereo = [silence_stereo_48k(1000), speech, silence_stereo_48k(2000)].concat();

    let gated = vad_gated_frames(&stereo);
    println!(
        "fishaudio_stt_pushed_ms {}/{}",
        gated.pushed_ms, gated.total_ms
    );
    assert!(gated.starts >= 1, "the VAD never detected the phrase");
    assert!(
        gated.pushed_ms < gated.total_ms - 1000,
        "the silence padding was streamed: {} of {} ms",
        gated.pushed_ms,
        gated.total_ms
    );

    let mut stt = FishAudioStt::new(&SttConfig {
        provider: SttVendor::Fishaudio,
        model: None,
        model_path: None,
        language: None,
        api_key: Some(key),
        endpoint: None,
        transport: None,
    })
    .unwrap_or_else(|err| panic!("stt config: {err}"));
    stt.start()
        .await
        .unwrap_or_else(|err| panic!("stt start: {err}"));
    assert!(
        stt.wait_ready(Duration::from_secs(10)).await.unwrap(),
        "session_started did not arrive within 10 s"
    );

    // Frame n is due at anchor + n x 20 ms: no bursts, no drift.
    let anchor = tokio::time::Instant::now();
    for (n, frame) in gated.frames_16k.iter().enumerate() {
        tokio::time::sleep_until(anchor + Duration::from_millis(20 * n as u64)).await;
        stt.push_audio(frame.clone())
            .await
            .unwrap_or_else(|err| panic!("push_audio: {err}"));
    }

    let committed_at = Instant::now();
    stt.request_finalize()
        .await
        .unwrap_or_else(|err| panic!("request_finalize: {err}"));
    let mut transcript = String::new();
    let mut first_final_ms = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no FinalizeDone within 10 s of the commit"
        );
        match stt.poll_output().await {
            Ok(Some(SttOutput::Transcript(SttTranscript::Final(text)))) => {
                first_final_ms.get_or_insert(committed_at.elapsed().as_millis());
                transcript.push(' ');
                transcript.push_str(&text);
            }
            Ok(Some(SttOutput::FinalizeDone)) => break,
            Ok(Some(SttOutput::Transcript(SttTranscript::Partial(_)))) => {
                panic!("Fish STT returned a partial")
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(err) => panic!("stt error: {err}"),
        }
    }
    let commit_to_final_ms = first_final_ms.unwrap_or_else(|| committed_at.elapsed().as_millis());
    println!("fishaudio_stt_commit_to_final_ms {commit_to_final_ms}");
    stt.stop().await.unwrap();

    let matched = matched_words(&transcript, &STT_WORDS);
    assert!(
        matched >= 7,
        "matched {matched} of {} words (transcript not printed)",
        STT_WORDS.len()
    );
}
