//! Replays WAV files (16 kHz mono s16le) through the same Sherpa online recognizer settings the
//! voice agent uses, and prints the recognized text. Use it on the `stt-in-*.wav` files that the
//! test audio tap (`SPEECH_TEST_AUDIO_TAP_DIR`) kept for a failed roundtrip turn.
//!
//! ```text
//! cargo run -p node-webrtc-rust-vendor-sherpa-onnx --example stt_replay -- \
//!   <stt-model-dir> <file.wav>...
//! ```
//!
//! Output: one line per file, `<file>\t<text>`.
//!
//! The recognizer settings replicate `create_online_recognizer` in `src/loader.rs` (transducer
//! encoder/decoder/joiner/tokens from the model dir, `greedy_search`, endpoint off, 1 thread);
//! that loader is crate-private.

use std::fs;
use std::path::{Path, PathBuf};

use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig};

const SAMPLE_RATE: i32 = 16_000;
/// 20 ms of 16 kHz audio.
const CHUNK_SAMPLES: usize = 320;

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

fn create_recognizer(model_dir: &Path) -> Result<OnlineRecognizer, String> {
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
fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
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

fn recognize(recognizer: &OnlineRecognizer, samples: &[f32]) -> String {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: stt_replay <stt-model-dir> <file.wav>...");
        std::process::exit(2);
    }
    let recognizer = match create_recognizer(&PathBuf::from(&args[0])) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stt_replay: {e}");
            std::process::exit(1);
        }
    };
    let mut failed = false;
    for file in &args[1..] {
        match read_wav(Path::new(file)) {
            Ok(samples) => println!("{file}\t{}", recognize(&recognizer, &samples)),
            Err(e) => {
                eprintln!("stt_replay: {file}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
