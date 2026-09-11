//! Prove Zipformer STT can emit a transcript while Whisper LID identify is still in flight.
//!
//! Kept `#[ignore]` for default `cargo test` (needs STT + LID + TTS weights). CI runs via
//! `scripts/ci/run-sherpa-example-ci.sh` after `ensure_sherpa_models`. Run with `--test-threads=1`.

use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    LanguageIdConfig, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::time::{sleep, timeout};

fn stt_config(model_path: String) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        language: Some("en".into()),
        api_key: None,
    }
}

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
    }
}

fn lid_config(model_path: String) -> LanguageIdConfig {
    LanguageIdConfig {
        enabled: Some(true),
        model_path: Some(model_path),
        allowlist: None,
        min_speech_ms: None,
        continuous: None,
    }
}

async fn synthesize_speech_pcm_16k() -> Bytes {
    let tts_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(tts_path))
        .expect("factory should create TTS");
    let chunks = tts
        .synthesize("one two three four five six seven eight nine ten")
        .await
        .expect("piper synth");
    let mut stereo = Vec::new();
    for chunk in chunks {
        stereo.extend_from_slice(chunk.pcm.as_ref());
    }
    let mono = stereo_48k_to_mono_16k(&stereo);
    i16_samples_to_bytes(&mono)
}

fn repeat_pcm(base: &Bytes, repeats: usize) -> Bytes {
    let mut out = Vec::with_capacity(base.len() * repeats);
    for _ in 0..repeats {
        out.extend_from_slice(base);
    }
    Bytes::from(out)
}

fn transcript_nonempty(transcript: &SttTranscript) -> bool {
    match transcript {
        SttTranscript::Partial(text) | SttTranscript::Final(text) => !text.trim().is_empty(),
    }
}

#[tokio::test]
#[ignore = "requires SHERPA_STT_MODEL_PATH + SHERPA_LID_MODEL_PATH (+ TTS for speech PCM)"]
async fn zipformer_partial_while_whisper_lid_in_flight() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let lid_path = std::env::var("SHERPA_LID_MODEL_PATH").expect("set SHERPA_LID_MODEL_PATH");

    let speech_pcm = synthesize_speech_pcm_16k().await;
    assert!(
        speech_pcm.len() >= 3200,
        "expected at least 100ms of 16k mono speech from Piper"
    );
    let lid_pcm = repeat_pcm(&speech_pcm, 12);

    let factory = SherpaFactory;
    let mut stt = factory
        .create_stt(&stt_config(stt_path))
        .expect("create_stt");
    stt.start().await.expect("stt start");

    let lid = factory
        .create_language_id(&lid_config(lid_path))
        .expect("create_language_id")
        .expect("lid provider");
    let lid_handle = tokio::spawn(async move { lid.identify(lid_pcm, 16_000).await });
    sleep(Duration::from_millis(5)).await;
    assert!(
        !lid_handle.is_finished(),
        "Whisper LID should still be running before STT push"
    );

    const CHUNK_BYTES: usize = 3200;
    let mut offset = 0usize;
    let deadline = Duration::from_secs(20);
    let result = timeout(deadline, async {
        let mut saw_transcript_while_lid_pending = false;
        while offset < speech_pcm.len() {
            let end = (offset + CHUNK_BYTES).min(speech_pcm.len());
            stt.push_audio(speech_pcm.slice(offset..end))
                .await
                .expect("push_audio");
            offset = end;

            if let Some(transcript) = stt.poll_transcript().await.expect("poll_transcript") {
                if transcript_nonempty(&transcript) {
                    if !lid_handle.is_finished() {
                        saw_transcript_while_lid_pending = true;
                        break;
                    }
                    panic!(
                        "STT transcript arrived only after LID completed — ORT sessions may still be serializing"
                    );
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
        saw_transcript_while_lid_pending
    })
    .await;

    stt.stop().await.expect("stt stop");

    match result {
        Ok(true) => {}
        Ok(false) => panic!(
            "no non-empty STT partial/final before LID completed within pushed audio — parallel isolation failed"
        ),
        Err(_) => panic!("timed out waiting for STT partial while LID in flight"),
    }

    lid_handle
        .await
        .expect("lid task join")
        .expect("lid result");
}
