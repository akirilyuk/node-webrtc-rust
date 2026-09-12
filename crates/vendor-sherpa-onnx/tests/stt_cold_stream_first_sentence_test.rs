//! Cold inbound stream: the listener's first-ever inbound frames are the talker's TTS onset.
//!
//! Reproduces the Sherpa `start:roundtrip-counting-echo-lid` CI miss where Agent1 heard
//! `"one two three …"` (no `echo.` prefix): nothing was written to Agent2's outbound track
//! before the echo TTS, so Agent1's STT pre-roll held only the TTS onset (~240 ms) and the
//! Zipformer stream started exactly at the first spoken sample with no lead-in context.
//!
//! Kept `#[ignore]` (needs Kroko STT + Piper TTS weights). Run:
//!
//! ```bash
//! SHERPA_STT_MODEL_PATH=examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 \
//! SHERPA_TTS_MODEL_PATH=examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low \
//! cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stt_cold_stream_first_sentence_test -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `STT_COLD_RENDERS` (default 6) controls how many independent Piper renders are checked
//! (VITS noise makes every render different). `STT_COLD_EXPERIMENT=1` only prints the
//! per-render cold vs warm transcripts without asserting.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::time::sleep;

const FRAME_MS: u32 = 20;
const STEREO_48K_FRAME_BYTES: usize = 48_000 / 1000 * FRAME_MS as usize * 2 * 2; // 3840
const ECHO_PHRASE: &str = "echo. One, two, three, four, five six seven eight nine ten";
/// Same VAD shape as `buildLocalSherpaVoiceConfig` + `echoVadConfig` in the Sherpa examples
/// (energy VAD 0.05, 200 ms min speech, 500 ms pad → 700 ms pre-roll ring).
fn harness_vad_config() -> VadConfig {
    let mut vad = VadConfig::default();
    vad.provider = "energy".into();
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 200;
    vad.speech_pad_ms = 500;
    vad.min_silence_duration_ms = 1300;
    vad.gate_stt = true;
    vad.gate_stt_open_on_pending = true;
    vad.stt_gate_hold_ms = 1000;
    vad.barge_in.enabled = false;
    vad
}

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

#[derive(Debug, Default)]
struct Transcript {
    partials: Vec<String>,
    finals: Vec<String>,
}

impl Transcript {
    fn best(&self) -> String {
        self.finals
            .iter()
            .chain(self.partials.iter())
            .max_by_key(|t| t.trim().len())
            .cloned()
            .unwrap_or_default()
    }

    fn heard_echo(&self) -> bool {
        self.best().to_lowercase().contains("echo")
    }
}

/// Feed one listener agent: optional zero lead-in frames, then the TTS render, then zeros
/// until `user_speech_final` (or a frame cap).
async fn run_listener(stt_model_path: &str, tts_pcm: &[u8], lead_in_frames: usize) -> Transcript {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::LocalSherpa, Arc::new(SherpaFactory));
    let config = VoiceAgentConfig {
        stt: Some(stt_config(stt_model_path.to_string())),
        tts: None,
        vad: harness_vad_config(),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut transcript = Transcript::default();
    let silent = Bytes::from(vec![0_u8; STEREO_48K_FRAME_BYTES]);
    let mut collect = |transcript: &mut Transcript| {
        while let Ok(event) = rx.try_recv() {
            match event.kind {
                SpeechEventKind::UserSpeechPartial => {
                    transcript.partials.push(event.text.unwrap_or_default())
                }
                SpeechEventKind::UserSpeechFinal => {
                    transcript.finals.push(event.text.unwrap_or_default())
                }
                _ => {}
            }
        }
    };

    for _ in 0..lead_in_frames {
        agent
            .process_inbound_pcm(silent.clone(), FRAME_MS)
            .await
            .unwrap();
        collect(&mut transcript);
    }
    for frame in tts_pcm.chunks(STEREO_48K_FRAME_BYTES) {
        if frame.len() < STEREO_48K_FRAME_BYTES {
            break;
        }
        agent
            .process_inbound_pcm(Bytes::copy_from_slice(frame), FRAME_MS)
            .await
            .unwrap();
        collect(&mut transcript);
    }
    // Trailing zeros: VAD SpeechEnd + gate hold + endpoint tail + finalize (all frame-driven).
    for _ in 0..400 {
        agent
            .process_inbound_pcm(silent.clone(), FRAME_MS)
            .await
            .unwrap();
        collect(&mut transcript);
        if !transcript.finals.is_empty() {
            break;
        }
        sleep(Duration::from_millis(2)).await;
    }
    agent.stop().await.ok();
    transcript
}

#[tokio::test]
#[ignore = "requires SHERPA_STT_MODEL_PATH (Kroko Zipformer) and SHERPA_TTS_MODEL_PATH (Piper)"]
async fn cold_inbound_stream_hears_first_tts_sentence() {
    let stt_model_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let tts_model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let renders: usize = std::env::var("STT_COLD_RENDERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let experiment_only = std::env::var("STT_COLD_EXPERIMENT").ok().as_deref() == Some("1");
    // Every render must be a fresh Piper pass — cache would collapse them into one sample.
    let _cache = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "0");

    let tts = SherpaFactory
        .create_tts(&tts_config(tts_model_path))
        .expect("create sherpa tts");
    let _ = tts.synthesize("Warm up.").await;

    let mut cold_misses = Vec::new();
    let mut warm_misses = Vec::new();
    for render in 0..renders {
        let chunks = tts.synthesize(ECHO_PHRASE).await.expect("piper synthesis");
        let mut pcm = Vec::new();
        for chunk in &chunks {
            pcm.extend_from_slice(&chunk.pcm);
        }
        assert!(pcm.len() >= STEREO_48K_FRAME_BYTES * 50, "render too short");

        let warm_frames: usize = std::env::var("STT_COLD_WARM_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);
        let cold = run_listener(&stt_model_path, &pcm, 0).await;
        let warm = run_listener(&stt_model_path, &pcm, warm_frames).await;
        println!(
            "render {render}: cold={:?} (first partial {:?}) | warm={:?} (first partial {:?})",
            cold.best(),
            cold.partials.first(),
            warm.best(),
            warm.partials.first(),
        );
        if !cold.heard_echo() {
            cold_misses.push((render, cold.best()));
        }
        if !warm.heard_echo() {
            warm_misses.push((render, warm.best()));
        }
        if !experiment_only {
            assert_eq!(
                cold.best(),
                warm.best(),
                "render {render}: cold and warm must match after pre-roll pad (cold={:?}, warm={:?})",
                cold.best(),
                warm.best()
            );
        }
    }

    println!(
        "summary: renders={renders} cold_misses={} warm_misses={}",
        cold_misses.len(),
        warm_misses.len()
    );
}
