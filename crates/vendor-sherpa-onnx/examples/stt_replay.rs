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

use std::path::{Path, PathBuf};

#[path = "support/mod.rs"]
mod support;

use support::{create_recognizer, read_wav, recognize};

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
