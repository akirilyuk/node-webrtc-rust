//! Concurrent Zipformer decode must match serial decode of the same PCM.
//!
//! `#[ignore]` for default `cargo test` (needs STT + TTS weights). CI runs it via
//! `scripts/ci/run-sherpa-example-ci.sh` after `ensure_sherpa_models`. Run with `--test-threads=1`.

use std::sync::Arc;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::sync::Barrier;

const PHRASES: [&str; 4] = [
    "alpha one two three",
    "bravo four five six",
    "delta seven eight nine",
    "echo ten eleven twelve",
];
const ITERATIONS: usize = 10;
/// 60 ms of 16 kHz mono s16le.
const CHUNK_BYTES: usize = 1920;

fn stt_config(model_path: String) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        language: Some("en".into()),
        api_key: None,
        endpoint: None,
    }
}

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
    }
}

async fn synthesize_speech_pcm_16k(text: &str) -> Bytes {
    let tts_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(tts_path))
        .expect("factory should create TTS");
    let chunks = tts.synthesize(text).await.expect("piper synth");
    let mut stereo = Vec::new();
    for chunk in chunks {
        stereo.extend_from_slice(chunk.pcm.as_ref());
    }
    let mono = stereo_48k_to_mono_16k(&stereo);
    i16_samples_to_bytes(&mono)
}

fn collect_finals(stt_out: &mut Vec<String>, transcript: Option<SttTranscript>) -> bool {
    match transcript {
        Some(SttTranscript::Final(text)) => {
            stt_out.push(text);
            true
        }
        Some(SttTranscript::Partial(_)) => true,
        None => false,
    }
}

async fn transcribe(cfg: &SttConfig, pcm: &[u8], barrier: Option<Arc<Barrier>>) -> String {
    let factory = SherpaFactory;
    let mut stt = factory.create_stt(cfg).expect("create_stt");
    stt.start().await.expect("stt start");
    if let Some(barrier) = barrier {
        barrier.wait().await;
    }
    let mut finals: Vec<String> = Vec::new();
    for chunk in pcm.chunks(CHUNK_BYTES) {
        stt.push_audio(Bytes::copy_from_slice(chunk))
            .await
            .expect("push_audio");
        while collect_finals(&mut finals, stt.poll_transcript().await.expect("poll")) {}
    }
    stt.finalize_utterance().await.expect("finalize_utterance");
    while collect_finals(&mut finals, stt.poll_transcript().await.expect("poll")) {}
    stt.stop().await.expect("stt stop");
    finals
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
async fn concurrent_decode_matches_serial() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let cfg = stt_config(stt_path);

    // Synthesize each phrase once; reuse the exact bytes everywhere.
    let mut pcms: Vec<Bytes> = Vec::new();
    for phrase in PHRASES {
        let pcm = synthesize_speech_pcm_16k(phrase).await;
        assert!(
            pcm.len() >= 3200,
            "phrase {phrase:?} produced too little PCM"
        );
        pcms.push(pcm);
    }

    let mut reference: Vec<String> = Vec::new();
    for (idx, pcm) in pcms.iter().enumerate() {
        let text = transcribe(&cfg, pcm.as_ref(), None).await;
        assert!(
            !text.is_empty(),
            "serial reference for phrase {idx} ({:?}) is empty",
            PHRASES[idx]
        );
        reference.push(text);
    }

    for iteration in 0..ITERATIONS {
        let barrier = Arc::new(Barrier::new(PHRASES.len() * 2));
        let mut handles = Vec::new();
        for (phrase_idx, pcm) in pcms.iter().enumerate() {
            for _copy in 0..2 {
                let cfg = cfg.clone();
                let pcm = pcm.clone();
                let barrier = Arc::clone(&barrier);
                handles.push((
                    phrase_idx,
                    tokio::spawn(
                        async move { transcribe(&cfg, pcm.as_ref(), Some(barrier)).await },
                    ),
                ));
            }
        }
        for (phrase_idx, handle) in handles {
            let actual = handle.await.expect("transcribe task");
            assert_eq!(
                actual, reference[phrase_idx],
                "iteration {iteration} phrase {phrase_idx}: reference={:?} actual={actual:?}",
                reference[phrase_idx]
            );
        }
    }
}
