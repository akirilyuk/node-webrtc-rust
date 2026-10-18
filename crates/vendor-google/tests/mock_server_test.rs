#![cfg(feature = "live")]
//! Offline checks of the Google REST TTS path against a local server: the WAV header of the
//! LINEAR16 reply is not audio, and 1 s of vendor audio is 1 s of 48 kHz stereo at any rate it
//! declares (catalog `audio.sample-rate-mismatch`).
//!
//! `StreamingSynthesize` and V2 `StreamingRecognize` are gRPC over TLS to Google; they cannot be
//! served by the plain-HTTP mock here. Their conversion and bounds are covered by the unit tests
//! of `tts_live::collect_stream` (24 kHz in, 1 s is 192000 bytes out; no answer, stall, trickle),
//! and of `stt_live::V2StreamHandle::finalize_stream`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use node_webrtc_rust_vendor_google::GoogleTts;
use node_webrtc_rust_vendor_live_support::http_mock::{
    pcm_16bit_mono, wav_16bit_mono, MockHttp, Reply,
};
use node_webrtc_rust_vendor_live_support::tts::assert_chunks_match_source;
use tokio::sync::mpsc;

fn tts(url: &str) -> GoogleTts {
    GoogleTts::new(&TtsConfig {
        provider: TtsVendor::Google,
        model: None,
        model_path: None,
        voice: Some("en-US-Neural2-A".into()),
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    })
    .unwrap()
    .__with_rest_url_override(format!("{url}/v1/text:synthesize"))
}

fn json_body(audio: &[u8]) -> Vec<u8> {
    format!("{{\"audioContent\":\"{}\"}}", STANDARD.encode(audio)).into_bytes()
}

fn bytes_of(chunks: &[TtsAudioChunk]) -> usize {
    chunks.iter().map(|c| c.pcm.len()).sum()
}

/// "For LINEAR16 audio, we include the WAV header": 1 s at any declared rate is about 1000 ms
/// and 192000 bytes of 48 kHz stereo, and the header is not played.
#[tokio::test]
async fn one_second_of_wav_is_one_second_of_48k_stereo_at_every_rate() {
    for rate in [16_000, 22_050, 24_000, 48_000] {
        let mock = MockHttp::start(Reply::ok(
            "application/json",
            json_body(&wav_16bit_mono(rate, 1000)),
        ))
        .await;
        let chunks = tts(&mock.base_url()).synthesize("hello").await.unwrap();
        assert_chunks_match_source(&format!("google_rest_{rate}"), &chunks, 1000);
        let bytes = bytes_of(&chunks);
        assert!(
            (191_000..=193_000).contains(&bytes),
            "{rate} Hz: {bytes} bytes"
        );
    }
}

/// The 44 header bytes are not audio: the first sample is the first tone sample, not RIFF text.
#[tokio::test]
async fn the_wav_header_is_not_played() {
    let mock = MockHttp::start(Reply::ok(
        "application/json",
        json_body(&wav_16bit_mono(48_000, 200)),
    ))
    .await;
    let chunks = tts(&mock.base_url()).synthesize("hello").await.unwrap();
    let first = &chunks[0].pcm;
    assert_eq!(i16::from_le_bytes([first[0], first[1]]), 800);
    assert_eq!(bytes_of(&chunks), 48_000 / 5 * 4);
}

/// REST audio reaches the sink in sink-sized pieces, once: sink bytes equal returned bytes.
#[tokio::test]
async fn rest_audio_is_cut_into_chunks_sent_to_the_sink_once() {
    let mock = MockHttp::start(Reply::ok(
        "application/json",
        json_body(&wav_16bit_mono(24_000, 1000)),
    ))
    .await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let chunks = tts(&mock.base_url())
        .synthesize_progressive("hello", Some(sink))
        .await
        .unwrap();
    let mut sent = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        sent.push(chunk);
    }
    assert!(chunks.len() >= 5, "{} chunks", chunks.len());
    assert_eq!(bytes_of(&sent), bytes_of(&chunks));
    assert_eq!(sent.len(), chunks.len());
}

/// A headerless body (not what the docs describe) is read as PCM at the requested 48 kHz.
#[tokio::test]
async fn a_body_without_a_header_is_read_at_the_requested_rate() {
    let mock = MockHttp::start(Reply::ok(
        "application/json",
        json_body(&pcm_16bit_mono(48_000, 1000)),
    ))
    .await;
    let chunks = tts(&mock.base_url()).synthesize("hello").await.unwrap();
    assert_chunks_match_source("google_rest_headerless", &chunks, 1000);
}
