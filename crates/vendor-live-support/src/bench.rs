//! Latency bench: one `vendor_bench <json>` line per sample, summarised by
//! `scripts/ci/vendor-bench-summary.mjs`. Never prints a key or a transcript.

use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider};
use serde::{Deserialize, Serialize};

use crate::audio::STT_WORDS;
use crate::stt::{measure_stt, open_stt, run_utterance};
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

/// The transports of `all` selected by `NWR_BENCH_TRANSPORT` (see [`transports_from`]).
pub fn transports_from_env<'a>(all: &[&'a str]) -> Vec<&'a str> {
    transports_from(std::env::var("NWR_BENCH_TRANSPORT").ok().as_deref(), all)
}

/// Testable core of [`transports_from_env`]: unset, empty or `all` keeps every transport;
/// otherwise the comma-separated values (trimmed) that are in `all`, in `all`'s order. Values
/// not in `all` are dropped.
pub fn transports_from<'a>(value: Option<&str>, all: &[&'a str]) -> Vec<&'a str> {
    let value = value.map(str::trim).unwrap_or("");
    if value.is_empty() || value == "all" {
        return all.to_vec();
    }
    let wanted: Vec<&str> = value.split(',').map(str::trim).collect();
    all.iter()
        .copied()
        .filter(|transport| wanted.contains(transport))
        .collect()
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

/// STT bench for a vendor whose session accepts several utterances (docs-confirmed). Each run is
/// a fresh provider: a `cold` sample (the open included in `open_ms`) and then a `warm` sample,
/// a second utterance on the same open session.
pub async fn bench_stt_warm(
    vendor: &str,
    transport: &str,
    model: &str,
    make_provider: impl Fn() -> Box<dyn SttProvider>,
    audio: &[u8],
    runs: usize,
) -> SpeechResult<()> {
    for run in 0..runs {
        let mut provider = make_provider();
        let outcome = async {
            let open_ms = open_stt(provider.as_mut()).await?;
            let mut cold = run_utterance(provider.as_mut(), audio, &STT_WORDS).await?;
            cold.open_ms = open_ms;
            let warm = run_utterance(provider.as_mut(), audio, &STT_WORDS).await?;
            Ok::<_, node_webrtc_rust_speech::error::SpeechError>((cold, warm))
        }
        .await;
        let http_version = provider.last_transport();
        let _ = provider.stop().await;
        let (cold, warm) = outcome?;
        for (phase, sample) in [("cold", cold), ("warm", warm)] {
            emit(&BenchRecord {
                vendor: vendor.into(),
                leg: "stt".into(),
                transport: transport.into(),
                model: model.into(),
                phase: phase.into(),
                text: "phrase".into(),
                run,
                http_version: http_version.clone(),
                metrics: serde_json::to_value(sample).expect("sample serializes"),
            });
        }
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

    /// Counts finalizes; every finalize answers with a final for the phrase.
    struct Fake {
        queue: std::collections::VecDeque<node_webrtc_rust_speech::pipeline::SttTranscript>,
        finalizes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        starts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SttProvider for Fake {
        fn vendor_name(&self) -> &'static str {
            "fake"
        }
        async fn start(&mut self) -> SpeechResult<()> {
            self.starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn stop(&mut self) -> SpeechResult<()> {
            Ok(())
        }
        async fn push_audio(&mut self, _pcm: bytes::Bytes) -> SpeechResult<()> {
            Ok(())
        }
        async fn poll_transcript(
            &mut self,
        ) -> SpeechResult<Option<node_webrtc_rust_speech::pipeline::SttTranscript>> {
            Ok(self.queue.pop_front())
        }
        async fn finalize_utterance(&mut self) -> SpeechResult<()> {
            self.finalizes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.queue
                .push_back(node_webrtc_rust_speech::pipeline::SttTranscript::Final(
                    crate::audio::STT_PHRASE.into(),
                ));
            Ok(())
        }
        fn emits_partials(&self) -> bool {
            false
        }
    }

    #[tokio::test(start_paused = true)]
    async fn warm_bench_runs_two_utterances_on_one_open_session() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let finalizes = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let (f, st) = (finalizes.clone(), starts.clone());
        bench_stt_warm(
            "fake",
            "websocket",
            "m",
            move || {
                Box::new(Fake {
                    queue: Default::default(),
                    finalizes: f.clone(),
                    starts: st.clone(),
                })
            },
            &crate::stt_fixture_stereo_48k(),
            2,
        )
        .await
        .unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2, "one open per run");
        assert_eq!(finalizes.load(Ordering::SeqCst), 4, "cold + warm per run");
    }

    #[test]
    fn transports_from_selects_in_the_order_of_all() {
        let all = ["websocket", "http"];
        assert_eq!(transports_from(None, &all), vec!["websocket", "http"]);
        assert_eq!(
            transports_from(Some("all"), &all),
            vec!["websocket", "http"]
        );
        assert_eq!(transports_from(Some(" http "), &all), vec!["http"]);
        assert_eq!(
            transports_from(Some("http,websocket"), &all),
            vec!["websocket", "http"]
        );
        assert!(transports_from(Some("grpc"), &all).is_empty());
    }

    #[test]
    fn texts_have_the_expected_size() {
        assert_eq!(SHORT.split_whitespace().count(), 3);
        assert_eq!(SENTENCE.split_whitespace().count(), 15);
    }
}
