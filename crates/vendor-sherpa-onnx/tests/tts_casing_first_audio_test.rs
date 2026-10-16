//! Measures how the casing of the text after the first sentence changes local Piper/VITS
//! synthesis: time to the first progressive chunk, and the longest silent gap in the
//! audio between 150 ms and 1500 ms.
//!
//! STT finals often start lowercase, so the echo agent says `Okay. eight one six three ten`.
//! Hypothesis: espeak-ng (inside the Piper engine) does not end a sentence at ". e", so the
//! engine renders one sentence (late first audio, no pause after "Okay"), while ". E" makes two.
//!
//! Kept `#[ignore]` (needs a Piper/VITS bundle). Run:
//!
//! ```text
//! SHERPA_TTS_CASING_MODEL_DIR=/abs/path/vits-piper-en_US-amy-medium \
//!   cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_casing_first_audio_test \
//!   -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::sync::mpsc;

const RUNS: usize = 5;
const FRAME_MS: usize = 20;
/// Stereo 48 kHz s16le: 48 frames/ms x 2 channels x 2 bytes.
const FRAME_BYTES: usize = 48 * FRAME_MS * 2 * 2;
const SILENT_RMS: f64 = 300.0;
const WINDOW_START_MS: usize = 150;
const WINDOW_END_MS: usize = 1500;

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

struct Measurement {
    first_chunk_ms: f64,
    longest_silence_ms: usize,
}

fn frame_rms(frame: &[u8]) -> f64 {
    let samples: Vec<f64> = frame
        .chunks_exact(2)
        .map(|b| f64::from(i16::from_le_bytes([b[0], b[1]])))
        .collect();
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
}

/// Longest run of silent 20 ms frames whose start lies inside the 150..1500 ms window.
fn longest_silence_ms(pcm: &[u8]) -> usize {
    let first = WINDOW_START_MS / FRAME_MS;
    let last = WINDOW_END_MS / FRAME_MS;
    let mut longest = 0usize;
    let mut run = 0usize;
    for (index, frame) in pcm.chunks_exact(FRAME_BYTES).enumerate() {
        if index < first || index >= last {
            continue;
        }
        if frame_rms(frame) < SILENT_RMS {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest * FRAME_MS
}

async fn measure(tts: &dyn TtsProvider, text: &str) -> Measurement {
    let (tx, mut rx) = mpsc::unbounded_channel::<TtsAudioChunk>();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let start = Instant::now();
    let synth = async {
        tts.synthesize_progressive(text, Some(sink))
            .await
            .expect("synthesize_progressive");
    };
    let collect = async {
        let mut first_chunk_ms = None;
        let mut pcm: Vec<u8> = Vec::new();
        while let Some(chunk) = rx.recv().await {
            if first_chunk_ms.is_none() {
                first_chunk_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            pcm.extend_from_slice(&chunk.pcm);
        }
        (first_chunk_ms.expect("no chunk reached the sink"), pcm)
    };
    let (_, (first_chunk_ms, pcm)) = tokio::join!(synth, collect);
    Measurement {
        first_chunk_ms,
        longest_silence_ms: longest_silence_ms(&pcm),
    }
}

fn median_f64(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

fn median_usize(values: &mut [usize]) -> usize {
    values.sort_unstable();
    values[values.len() / 2]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_TTS_CASING_MODEL_DIR with a Piper/VITS bundle"]
async fn casing_after_first_sentence_changes_first_audio_and_pause() {
    let Ok(model_dir) = std::env::var("SHERPA_TTS_CASING_MODEL_DIR") else {
        eprintln!("SHERPA_TTS_CASING_MODEL_DIR is not set; skipping");
        return;
    };
    let _stream_on = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");

    let config = TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_dir),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
    };
    let tts = SherpaFactory.create_tts(&config).expect("create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("p-casing".into()),
        ..Default::default()
    });

    let lower = "Okay. eight one six three ten";
    let upper = "Okay. Eight one six three ten";

    // Warm-up: loads the engine, not measured.
    let _ = measure(tts.as_ref(), upper).await;

    let mut lower_first = Vec::new();
    let mut lower_silence = Vec::new();
    let mut upper_first = Vec::new();
    let mut upper_silence = Vec::new();
    for _ in 0..RUNS {
        let m = measure(tts.as_ref(), lower).await;
        lower_first.push(m.first_chunk_ms);
        lower_silence.push(m.longest_silence_ms);
        let m = measure(tts.as_ref(), upper).await;
        upper_first.push(m.first_chunk_ms);
        upper_silence.push(m.longest_silence_ms);
    }

    println!("lowercase {lower:?}: first_chunk_ms={lower_first:.0?} silence_ms={lower_silence:?}");
    println!("uppercase {upper:?}: first_chunk_ms={upper_first:.0?} silence_ms={upper_silence:?}");
    let lf = median_f64(&mut lower_first);
    let uf = median_f64(&mut upper_first);
    let ls = median_usize(&mut lower_silence);
    let us = median_usize(&mut upper_silence);
    println!("MEDIAN lowercase: first_chunk_ms={lf:.0} longest_silence_ms={ls}");
    println!("MEDIAN uppercase: first_chunk_ms={uf:.0} longest_silence_ms={us}");
    println!(
        "DELTA first_chunk_ms={:.0} longest_silence_ms={}",
        lf - uf,
        ls as i64 - us as i64
    );
}
