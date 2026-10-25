//! Helpers shared by the `stt_replay` and `phrase_sweep` examples: the Sherpa online recognizer
//! with the voice agent's settings, WAV reading, feeding a recognizer, and the offline TTS engine
//! with nwr's settings.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

use sherpa_onnx::{GenerationConfig, OfflineTts, OnlineRecognizer, OnlineRecognizerConfig};

/// Recognizer input rate.
pub const SAMPLE_RATE: i32 = 16_000;
/// 20 ms of 16 kHz audio.
pub const CHUNK_SAMPLES: usize = 320;

fn find_onnx(dir: &Path, keyword: &str) -> Result<String, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let lower = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if path.is_file() && lower.ends_with(".onnx") && lower.contains(keyword) {
            return Ok(path.to_string_lossy().into_owned());
        }
    }
    Err(format!("no {keyword} .onnx in {}", dir.display()))
}

/// The recognizer settings replicate `create_online_recognizer` in `src/loader.rs` (transducer
/// encoder/decoder/joiner/tokens from the model dir, `greedy_search`, endpoint off, 1 thread);
/// that loader is crate-private.
pub fn create_recognizer(model_dir: &Path) -> Result<OnlineRecognizer, String> {
    let tokens = model_dir.join("tokens.txt");
    if !tokens.is_file() {
        return Err(format!("no tokens.txt in {}", model_dir.display()));
    }
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.num_threads = 1;
    config.model_config.transducer.encoder = Some(find_onnx(model_dir, "encoder")?);
    config.model_config.transducer.decoder = Some(find_onnx(model_dir, "decoder")?);
    config.model_config.transducer.joiner = Some(find_onnx(model_dir, "joiner")?);
    config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
    config.enable_endpoint = false;
    config.decoding_method = Some("greedy_search".into());
    OnlineRecognizer::create(&config).ok_or_else(|| "failed to create OnlineRecognizer".to_string())
}

/// Samples of a 16 kHz mono s16le PCM WAV, normalized to f32.
pub fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|e| format!("read: {e}"))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12;
    let mut fmt_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            if body + 16 > bytes.len() {
                return Err("truncated fmt chunk".into());
            }
            let format = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            if format != 1 || channels != 1 || rate != 16_000 || bits != 16 {
                return Err(format!(
                    "expected 16 kHz mono s16le PCM, got format={format} channels={channels} rate={rate} bits={bits}"
                ));
            }
            fmt_ok = true;
        } else if id == b"data" {
            if !fmt_ok {
                return Err("data chunk before fmt chunk".into());
            }
            let end = (body + size).min(bytes.len());
            return Ok(bytes[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
                .collect());
        }
        pos = body + size + (size & 1);
    }
    Err("no data chunk".into())
}

pub fn recognize(recognizer: &OnlineRecognizer, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    for chunk in samples.chunks(CHUNK_SAMPLES) {
        stream.accept_waveform(SAMPLE_RATE, chunk);
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
    }
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    recognizer
        .get_result(&stream)
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default()
}

/// The offline TTS engine exactly as nwr builds it: the public `create_offline_tts` in
/// `src/loader.rs` (VITS `noise_scale` 0.667, `noise_scale_w` 0.8, `length_scale` 1.0, rule FSTs and
/// espeak data resolved from the model dir, `SHERPA_TTS_NUM_THREADS` or 2 threads).
pub fn create_tts(model_dir: &Path) -> Result<OfflineTts, String> {
    use node_webrtc_rust_speech::config::{SentenceCase, TtsConfig, TtsVendor};
    let config = TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_dir.to_string_lossy().into_owned()),
        voice: None,
        api_key: None,
        endpoint: None,
        transport: None,
        sentence_case: SentenceCase::Auto,
    };
    node_webrtc_rust_vendor_sherpa_onnx::create_offline_tts(&config).map_err(|e| e.to_string())
}

/// Per-synthesis settings nwr passes to `generate_with_config` in `src/tts.rs`
/// (`synthesize_sentence`): speaker id from `parse_speaker_id` (0 when the voice is unset) and
/// speed from `parse_speed` (`SHERPA_TTS_SPEED`, else 1.0, clamped to 0.2..=2.0).
pub fn generation_config() -> GenerationConfig {
    let speed = std::env::var("SHERPA_TTS_SPEED")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1.0)
        .clamp(0.2, 2.0);
    GenerationConfig {
        sid: 0,
        speed,
        ..Default::default()
    }
}

/// Synthesize `text` with the engine calls nwr's Sherpa TTS makes for it: whitespace
/// normalization (`normalize_phrase_text`), then [`engine_pieces`] (sentence-start capitalization
/// under `SentenceCase::Auto`, split into sentences with a short first piece), one engine call per
/// piece, samples concatenated back to back as `synthesize_miss` streams them. Returns the samples
/// and the engine sample rate.
///
/// [`engine_pieces`]: node_webrtc_rust_vendor_sherpa_onnx::engine_pieces
pub fn synthesize(tts: &OfflineTts, text: &str) -> Result<(Vec<f32>, i32), String> {
    use node_webrtc_rust_speech::config::SentenceCase;
    use node_webrtc_rust_speech::tts_cache::normalize_phrase_text;
    use node_webrtc_rust_vendor_sherpa_onnx::engine_pieces;
    let mut samples = Vec::new();
    for piece in engine_pieces(&normalize_phrase_text(text), SentenceCase::Auto) {
        let audio = tts
            .generate_with_config(
                &piece,
                &generation_config(),
                None::<fn(&[f32], f32) -> bool>,
            )
            .ok_or_else(|| format!("OfflineTts generation returned no audio for {piece:?}"))?;
        samples.extend_from_slice(audio.samples());
    }
    Ok((samples, tts.sample_rate()))
}
