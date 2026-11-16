//! Whisper language scores + allowlist decision on dumped real-path clips (need Whisper tiny).
//!
//! `#[ignore]` for default `cargo test`; run with `SHERPA_LID_MODEL_PATH` set (CI:
//! `scripts/ci/run-sherpa-example-ci.sh`). Fixtures in `tests/fixtures/lid-allowed-set/`
//! (`manifest.json` has the product-runtime scores): three real-path Spanish utterances (Opus +
//! rnnoise + VAD) whose unrestricted Whisper-tiny pick is `ja`, one English, one German (Piper).
//! Every fixture has a top-1/top-2 margin >= 0.5, so scorer and sherpa must agree on the top.

use std::path::PathBuf;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{decide_language_among_allowed, LanguageIdConfig};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::{SherpaFactory, WhisperLanguageScorer};

fn config(allowlist: Option<Vec<&str>>) -> LanguageIdConfig {
    LanguageIdConfig {
        enabled: None,
        model_path: Some(
            std::env::var("SHERPA_LID_MODEL_PATH").expect("set SHERPA_LID_MODEL_PATH"),
        ),
        allowlist: allowlist.map(|l| l.into_iter().map(String::from).collect()),
        allowlist_max_gap: None,
        allowlist_min_margin: None,
        min_speech_ms: None,
        continuous: None,
        timing: None,
        lid_max_clip_ms: None,
        lid_gate_max_wait_ms: None,
        tts_exclusion: None,
    }
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lid-allowed-set")
}

fn read_wav_16k(name: &str) -> Vec<i16> {
    let bytes = std::fs::read(fixture_dir().join(name)).expect("fixture");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        16_000
    );
    bytes[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect()
}

/// (file, expected restricted decision with allowlist {en, es} at the default gap)
const CASES: &[(&str, &str)] = &[
    ("es-miss-1787-a150-utt2.wav", "es"),
    ("es-miss-785-a105-utt2.wav", "es"),
    ("es-miss-1787-a129-utt2.wav", "es"),
    ("22459-a143-utt1.wav", "en"),
    ("de-099.wav", "de"),
];

fn pcm_bytes(samples: &[i16]) -> Bytes {
    Bytes::from(
        samples
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect::<Vec<u8>>(),
    )
}

#[tokio::test]
#[ignore = "needs SHERPA_LID_MODEL_PATH (Whisper tiny)"]
async fn unrestricted_argmax_matches_sherpa_and_allowlist_decides() {
    let scorer = WhisperLanguageScorer::load_for_config(&config(None)).expect("load scorer");
    let factory = SherpaFactory;
    let plain = factory
        .create_language_id(&config(None))
        .expect("plain lid")
        .expect("provider");
    let restricted_cfg = config(Some(vec!["en", "es"]));
    let restricted = factory
        .create_language_id(&restricted_cfg)
        .expect("restricted lid")
        .expect("provider");
    for (file, expected) in CASES {
        let samples = read_wav_16k(file);
        let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
        let scores = scorer.language_scores(&floats).expect("scores");
        let top = scores
            .logits
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| scores.codes[i].clone())
            .unwrap();
        let mut order: Vec<usize> = (0..scores.logits.len()).collect();
        order.sort_by(|a, b| scores.logits[*b].total_cmp(&scores.logits[*a]));
        eprintln!(
            "{file}: top5 {:?}",
            order[..5]
                .iter()
                .map(|i| (scores.codes[*i].as_str(), scores.logits[*i]))
                .collect::<Vec<_>>()
        );
        let sherpa = plain
            .identify(pcm_bytes(&samples), 16_000)
            .await
            .expect("sherpa identify")
            .expect("sherpa result");
        eprintln!("{file}: sherpa={}", sherpa.language);
        assert_eq!(
            top, sherpa.language,
            "{file}: unrestricted argmax vs sherpa"
        );
        let decided =
            decide_language_among_allowed(&restricted_cfg, &scores.codes, &scores.logits).unwrap();
        assert_eq!(decided.language, *expected, "{file}: {decided:?}");
        let via_provider = restricted
            .identify(pcm_bytes(&samples), 16_000)
            .await
            .expect("restricted identify")
            .expect("restricted result");
        assert_eq!(
            via_provider.language, *expected,
            "{file}: provider {via_provider:?}"
        );
        assert_eq!(via_provider.raw_language.as_deref(), Some(top.as_str()));
    }
}
