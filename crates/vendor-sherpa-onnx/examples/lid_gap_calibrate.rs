//! Calibrate `languageId.allowlistMaxGap` on the product runtime (sherpa-onnx's bundled ONNX
//! Runtime, same thread count as the LID engine).
//!
//! usage: cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example lid_gap_calibrate --
//!          <whisper model dir> <allowlist csv, e.g. en,es> <wav file or dir>...
//!
//! Prints one JSON line per 16 kHz mono s16 WAV: unrestricted top and runner-up with their logit
//! margin, best allowlisted language and its gap to the top, and sherpa's own
//! `SpokenLanguageIdentification` pick on the same samples.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use node_webrtc_rust_speech::config::LanguageIdConfig;
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::{SherpaFactory, WhisperLanguageScorer};

fn read_wav_16k(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).expect("read wav");
    assert_eq!(&bytes[0..4], b"RIFF", "{}", path.display());
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        16_000,
        "{} must be 16 kHz",
        path.display()
    );
    bytes[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect()
}

fn collect(arg: &str, out: &mut Vec<PathBuf>) {
    let p = PathBuf::from(arg);
    if p.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&p)
            .expect("read dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|f| f.extension().is_some_and(|x| x == "wav"))
            .collect();
        files.sort();
        out.extend(files);
    } else {
        out.push(p);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        args.len() >= 3,
        "usage: <model dir> <allowlist csv> <wav|dir>..."
    );
    let allowed: Vec<String> = args[1].split(',').map(|c| c.trim().to_string()).collect();
    let config = LanguageIdConfig {
        enabled: None,
        model_path: Some(args[0].clone()),
        allowlist: None,
        allowlist_max_gap: None,
        allowlist_min_margin: None,
        min_speech_ms: None,
        continuous: None,
        timing: None,
        lid_max_clip_ms: None,
        lid_gate_max_wait_ms: None,
        tts_exclusion: None,
    };
    let scorer = WhisperLanguageScorer::load_for_config(&config).expect("load scorer");
    let sherpa = SherpaFactory
        .create_language_id(&config)
        .expect("sherpa lid")
        .expect("provider");
    let mut files = Vec::new();
    for a in &args[2..] {
        collect(a, &mut files);
    }
    for file in files {
        let samples = read_wav_16k(&file);
        let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
        let s = scorer.language_scores(&floats).expect("scores");
        let mut order: Vec<usize> = (0..s.logits.len()).collect();
        order.sort_by(|a, b| s.logits[*b].total_cmp(&s.logits[*a]));
        let best = order
            .iter()
            .copied()
            .find(|i| allowed.iter().any(|c| c == &s.codes[*i]))
            .expect("allowlist code in model");
        let pcm = Bytes::from(
            samples
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        let sh = sherpa
            .identify(pcm, 16_000)
            .await
            .expect("sherpa identify")
            .map(|r| r.language)
            .unwrap_or_default();
        // Logit of every allowlisted language, and the margin between the best and the
        // runner-up allowed language (how close the decision among the allowed set was).
        let mut allowed_logits: Vec<(String, f32)> = allowed
            .iter()
            .filter_map(|c| {
                s.codes
                    .iter()
                    .position(|x| x == c)
                    .map(|i| (c.clone(), s.logits[i]))
            })
            .collect();
        allowed_logits.sort_by(|a, b| b.1.total_cmp(&a.1));
        let allowed_margin = if allowed_logits.len() >= 2 {
            allowed_logits[0].1 - allowed_logits[1].1
        } else {
            f32::INFINITY
        };
        let allowed_json = allowed_logits
            .iter()
            .map(|(c, l)| format!("\"{c}\":{l:.4}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"file\":\"{}\",\"top\":\"{}\",\"second\":\"{}\",\"margin12\":{:.4},\"best\":\"{}\",\"gap\":{:.4},\"allowed\":{{{}}},\"allowed_margin\":{:.4},\"sherpa\":\"{}\"}}",
            file.display(),
            s.codes[order[0]],
            s.codes[order[1]],
            s.logits[order[0]] - s.logits[order[1]],
            s.codes[best],
            s.logits[order[0]] - s.logits[best],
            allowed_json,
            allowed_margin,
            sh
        );
    }
}
