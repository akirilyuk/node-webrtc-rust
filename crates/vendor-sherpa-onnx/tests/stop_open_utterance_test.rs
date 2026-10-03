//! `VoiceAgent::stop()` must complete promptly while an STT utterance is still open.
//!
//! Scenario (a listen-only client agent with local Sherpa STT + TTS + LID configured): inbound
//! speech produces STT partials, then inbound audio simply stops (no trailing silence frames, no
//! end-of-stream signal), and the host calls `stop()` (for example to swap the STT model).
//!
//! Kept `#[ignore]` (needs Zipformer STT, Piper TTS and Whisper LID weights). Run from
//! `node-webrtc-rust/`:
//!
//! ```bash
//! M=examples/voice-agent-local-sherpa/.models
//! SHERPA_STT_MODEL_PATH=$M/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 \
//! SHERPA_TTS_MODEL_PATH=$M/vits-piper-en_US-amy-low \
//! SHERPA_LID_MODEL_PATH=$M/sherpa-onnx-whisper-tiny \
//! cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stop_open_utterance_test -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `STOP_OPEN_ITERATIONS` (default 3) repeats each scenario.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    LanguageIdConfig, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::time::{sleep, timeout};

const FRAME_MS: u32 = 20;
const STEREO_48K_FRAME_BYTES: usize = 48_000 / 1000 * FRAME_MS as usize * 2 * 2;
const PHRASE: &str =
    "Okay. One, two, three, four, five, six, seven, eight, nine, ten, eleven, twelve.";
/// Upper bound for `stop()`. A healthy stop takes well under a second.
const STOP_BOUND: Duration = Duration::from_secs(10);

fn vad() -> VadConfig {
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

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("set {key}"))
}

async fn render_speech(tts_path: &str) -> Vec<u8> {
    let tts = SherpaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(tts_path.to_string()),
            voice: Some("0".into()),
            api_key: None,
            endpoint: None,
        })
        .expect("create tts");
    let chunks = tts.synthesize(PHRASE).await.expect("synthesize");
    let mut pcm = Vec::new();
    for chunk in &chunks {
        pcm.extend_from_slice(&chunk.pcm);
    }
    assert!(pcm.len() > STEREO_48K_FRAME_BYTES * 100, "render too short");
    pcm
}

/// Feeds `pcm` until the first STT partial, optionally waits `idle_before_stop`, then stops.
/// Returns how long `stop()` took, or `None` when it exceeded [`STOP_BOUND`].
async fn stop_with_open_utterance(
    pcm: &[u8],
    with_lid: bool,
    idle_before_stop: Duration,
    pump_silence: bool,
) -> Option<Duration> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::LocalSherpa, Arc::new(SherpaFactory));
    registry.register_tts(TtsVendor::LocalSherpa, Arc::new(SherpaFactory));
    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::LocalSherpa,
            model: None,
            model_path: Some(env("SHERPA_STT_MODEL_PATH")),
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(env("SHERPA_TTS_MODEL_PATH")),
            voice: Some("0".into()),
            api_key: None,
            endpoint: None,
        }),
        language_id: with_lid.then(|| LanguageIdConfig {
            enabled: Some(true),
            model_path: Some(env("SHERPA_LID_MODEL_PATH")),
            allowlist: Some(vec!["en".into(), "de".into()]),
            min_speech_ms: Some(800),
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: None,
        }),
        vad: vad(),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let mut partials = 0usize;
    'feed: for frame in pcm.chunks(STEREO_48K_FRAME_BYTES) {
        if frame.len() < STEREO_48K_FRAME_BYTES {
            break;
        }
        agent
            .process_inbound_pcm(Bytes::copy_from_slice(frame), FRAME_MS)
            .await
            .unwrap();
        sleep(Duration::from_millis(FRAME_MS as u64)).await;
        while let Ok(event) = rx.try_recv() {
            if event.kind == SpeechEventKind::UserSpeechPartial {
                partials += 1;
            }
        }
        // Stop mid-utterance, after a few partials, with the speaker still talking.
        if partials >= 3 {
            break 'feed;
        }
    }
    assert!(partials >= 1, "never produced an STT partial");
    // Host keeps pumping silent frames (the SDK injects a silence tail when the inbound RTP
    // stream ends) while `stop()` is requested; one `process_inbound_pcm` may be in flight.
    let agent = Arc::new(agent);
    let feeding = Arc::new(std::sync::atomic::AtomicBool::new(pump_silence));
    let pump = {
        let agent = Arc::clone(&agent);
        let feeding = Arc::clone(&feeding);
        tokio::spawn(async move {
            let silent = Bytes::from(vec![0_u8; STEREO_48K_FRAME_BYTES]);
            while feeding.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = agent.process_inbound_pcm(silent.clone(), FRAME_MS).await;
                sleep(Duration::from_millis(FRAME_MS as u64)).await;
            }
        })
    };
    sleep(idle_before_stop).await;

    let started = Instant::now();
    let result = match timeout(STOP_BOUND, agent.stop()).await {
        Ok(_) => Some(started.elapsed()),
        Err(_) => None,
    };
    feeding.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = timeout(Duration::from_secs(2), pump).await;
    result
}

async fn run_scenario(name: &str, with_lid: bool, idle: Duration, pump_silence: bool) {
    let iterations: usize = std::env::var("STOP_OPEN_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let pcm = render_speech(&env("SHERPA_TTS_MODEL_PATH")).await;
    for i in 0..iterations {
        let took = stop_with_open_utterance(&pcm, with_lid, idle, pump_silence).await;
        println!("{name} iteration {i}: stop() took {took:?}");
        let took = took.unwrap_or_else(|| {
            panic!("{name} iteration {i}: stop() did not complete within {STOP_BOUND:?}")
        });
        assert!(
            took < Duration::from_secs(5),
            "{name}: stop() took {took:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Sherpa STT + TTS + LID model paths"]
async fn stop_immediately_after_partial_with_lid() {
    run_scenario("immediate+lid", true, Duration::ZERO, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Sherpa STT + TTS + LID model paths"]
async fn stop_after_inbound_stall_with_lid() {
    run_scenario("stall+lid", true, Duration::from_millis(1300), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Sherpa STT + TTS model paths"]
async fn stop_after_inbound_stall_without_lid() {
    run_scenario("stall", false, Duration::from_millis(1300), false).await;
}

/// Silence keeps flowing after the last speech frame (SDK silence tail) and `stop()` lands while
/// VAD end-of-speech, gate hold, endpoint tail and finalize are in progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Sherpa STT + TTS + LID model paths"]
async fn stop_during_silence_tail_finalize_with_lid() {
    for ms in [100u64, 400, 800, 1200, 1500, 2000, 2500] {
        run_scenario(
            &format!("tail{ms}+lid"),
            true,
            Duration::from_millis(ms),
            true,
        )
        .await;
    }
}

/// Host keeps pumping speech then silence in real time while `stop()` is requested at several
/// offsets, so a `process_inbound_pcm` frame is in flight when `stop()` runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[ignore = "requires Sherpa STT + TTS + LID model paths"]
async fn stop_at_offsets_while_pumping() {
    let pcm = Arc::new(render_speech(&env("SHERPA_TTS_MODEL_PATH")).await);
    for offset_ms in [800u64, 2_700, 4_500, 6_400] {
        let took = stop_while_pumping(Arc::clone(&pcm), Duration::from_millis(offset_ms)).await;
        println!("pump: stop at +{offset_ms}ms took {took:?}");
        let took = took.unwrap_or_else(|| {
            panic!("stop() at +{offset_ms}ms did not complete within {STOP_BOUND:?}")
        });
        assert!(took < Duration::from_secs(5), "stop() took {took:?}");
    }
}

async fn stop_while_pumping(pcm: Arc<Vec<u8>>, stop_after: Duration) -> Option<Duration> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::LocalSherpa, Arc::new(SherpaFactory));
    registry.register_tts(TtsVendor::LocalSherpa, Arc::new(SherpaFactory));
    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::LocalSherpa,
            model: None,
            model_path: Some(env("SHERPA_STT_MODEL_PATH")),
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(env("SHERPA_TTS_MODEL_PATH")),
            voice: Some("0".into()),
            api_key: None,
            endpoint: None,
        }),
        language_id: Some(LanguageIdConfig {
            enabled: Some(true),
            model_path: Some(env("SHERPA_LID_MODEL_PATH")),
            allowlist: Some(vec!["en".into(), "de".into()]),
            min_speech_ms: Some(800),
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: None,
        }),
        vad: vad(),
        ..Default::default()
    };
    let agent = Arc::new(VoiceAgent::new(config, Arc::new(registry)).unwrap());
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let pump = {
        let agent = Arc::clone(&agent);
        let running = Arc::clone(&running);
        tokio::spawn(async move {
            let silent = Bytes::from(vec![0_u8; STEREO_48K_FRAME_BYTES]);
            for frame in pcm.chunks(STEREO_48K_FRAME_BYTES) {
                if !running.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                if frame.len() == STEREO_48K_FRAME_BYTES {
                    let _ = agent
                        .process_inbound_pcm(Bytes::copy_from_slice(frame), FRAME_MS)
                        .await;
                    sleep(Duration::from_millis(FRAME_MS as u64)).await;
                }
            }
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = agent.process_inbound_pcm(silent.clone(), FRAME_MS).await;
                sleep(Duration::from_millis(FRAME_MS as u64)).await;
            }
        })
    };
    sleep(stop_after).await;
    let started = Instant::now();
    let result = match timeout(STOP_BOUND, agent.stop()).await {
        Ok(_) => Some(started.elapsed()),
        Err(_) => None,
    };
    running.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = timeout(Duration::from_secs(2), pump).await;
    result
}
