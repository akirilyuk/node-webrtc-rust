//! Latency bench: one `vendor_bench <json>` line per sample, summarised by
//! `scripts/ci/vendor-bench-summary.mjs`. Never prints a key or a transcript.

use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider};
use serde::{Deserialize, Serialize};

use crate::audio::STT_WORDS;
use crate::stt::measure_stt;
use crate::tts::measure_tts;

/// Short reply (about 3 words).
pub const SHORT: &str = "Sure, one moment.";
/// A sentence of about 15 words.
pub const SENTENCE: &str =
    "Thanks for calling, I can help you check the status of your order right now.";

/// One measured sample.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchRecord {
    pub vendor: String,
    /// `"tts"` or `"stt"`.
    pub leg: String,
    pub transport: String,
    pub model: String,
    /// `"cold"` (new provider) or `"warm"` (same provider, pooled connection).
    pub phase: String,
    /// `"short"`, `"sentence"` (TTS) or `"phrase"` (STT).
    pub text: String,
    pub run: usize,
    /// From `last_transport()`: `h2`, `http/1.1`, `ws` or `grpc` when the vendor crate reports it.
    pub http_version: Option<String>,
    pub metrics: serde_json::Value,
}

/// Prints one `vendor_bench <json>` line.
pub fn emit(record: &BenchRecord) {
    println!(
        "vendor_bench {}",
        serde_json::to_string(record).expect("bench record serializes")
    );
}

/// Number of runs from `NWR_BENCH_RUNS` (default `default`).
pub fn runs_from_env(default: usize) -> usize {
    std::env::var("NWR_BENCH_RUNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// TTS bench. Per run and per text: a new provider, a cold sample, then a warm sample on the
/// same provider.
pub async fn bench_tts(
    vendor: &str,
    transport: &str,
    model: &str,
    make_provider: impl Fn() -> Box<dyn TtsProvider>,
    runs: usize,
) -> SpeechResult<()> {
    for run in 0..runs {
        for (label, text) in [("short", SHORT), ("sentence", SENTENCE)] {
            let provider = make_provider();
            for phase in ["cold", "warm"] {
                let sample = measure_tts(provider.as_ref(), text).await?;
                let http_version = provider.last_transport();
                emit(&BenchRecord {
                    vendor: vendor.into(),
                    leg: "tts".into(),
                    transport: transport.into(),
                    model: model.into(),
                    phase: phase.into(),
                    text: label.into(),
                    run,
                    http_version,
                    metrics: serde_json::to_value(&sample).expect("sample serializes"),
                });
            }
        }
    }
    Ok(())
}

/// STT bench. Each run is a fresh provider and one utterance, so every sample is `cold`.
/// A `warm` second utterance on the same provider is not measured: whether a provider accepts
/// another utterance after a finalize differs per vendor, so a vendor crate opts in with
/// [`crate::stt::run_utterance`] when its docs allow it.
pub async fn bench_stt(
    vendor: &str,
    transport: &str,
    model: &str,
    make_provider: impl Fn() -> Box<dyn SttProvider>,
    audio: &[u8],
    runs: usize,
) -> SpeechResult<()> {
    for run in 0..runs {
        let mut provider = make_provider();
        let sample = measure_stt(provider.as_mut(), audio, &STT_WORDS).await;
        let http_version = provider.last_transport();
        let _ = provider.stop().await;
        emit(&BenchRecord {
            vendor: vendor.into(),
            leg: "stt".into(),
            transport: transport.into(),
            model: model.into(),
            phase: "cold".into(),
            text: "phrase".into(),
            run,
            http_version,
            metrics: serde_json::to_value(sample?).expect("sample serializes"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_as_json() {
        let record = BenchRecord {
            vendor: "v".into(),
            leg: "tts".into(),
            transport: "http".into(),
            model: "m".into(),
            phase: "warm".into(),
            text: "short".into(),
            run: 2,
            http_version: Some("h2".into()),
            metrics: serde_json::json!({"ttfa_ms": 120}),
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: BenchRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
        assert!(json.contains("\"http_version\":\"h2\""));
    }

    #[test]
    fn texts_have_the_expected_size() {
        assert_eq!(SHORT.split_whitespace().count(), 3);
        assert_eq!(SENTENCE.split_whitespace().count(), 15);
    }
}
