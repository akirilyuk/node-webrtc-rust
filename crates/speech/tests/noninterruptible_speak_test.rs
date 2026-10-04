//! Per-utterance non-interruptible speech: `SendTextToTtsOptions { interruptible: false }`.
//!
//! VAD / STT-partial barge-in must not flush or cancel a protected job, jobs queued after it stay
//! interruptible, and explicit `flush_tts` / `stop` still stop it.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{sleep, Duration, Instant};

fn stereo_frame(loud: bool) -> Vec<u8> {
    let sample = if loud { i16::MAX / 3 } else { 0 };
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

fn mock_tts_duration_ms(text: &str) -> u32 {
    (text.len() as u32 * 50).clamp(100, 5000)
}

fn config() -> VoiceAgentConfig {
    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = false;
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = false;
    VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        vad,
        ..Default::default()
    }
}

async fn make_agent() -> (Arc<VoiceAgent>, Arc<Mutex<u32>>) {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(config(), Arc::new(registry)).unwrap();
    let written_ms: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let written = Arc::clone(&written_ms);
    // Pace outbound frames so barge-in can land mid-drain; block_in_place keeps inbound VAD running.
    let writer: PcmWriter = Arc::new(move |_pcm, ms| {
        *written.lock().unwrap() += ms;
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(4)));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, written_ms)
}

/// One user utterance: loud frames then silence so the next burst is a fresh SpeechStart edge.
async fn user_burst(agent: &VoiceAgent) {
    for _ in 0..8 {
        agent
            .process_inbound_pcm(Bytes::from(stereo_frame(true)), 20)
            .await
            .unwrap();
    }
    for _ in 0..6 {
        agent
            .process_inbound_pcm(Bytes::from(stereo_frame(false)), 20)
            .await
            .unwrap();
    }
}

fn drain_kinds(
    events: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
) -> Vec<SpeechEventKind> {
    let mut out = Vec::new();
    while let Ok(e) = events.try_recv() {
        out.push(e.kind);
    }
    out
}

/// Blocks until `kind` is observed (event-driven; no wall-clock guess about scheduling).
async fn wait_for_kind(
    events: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
    kind: SpeechEventKind,
    nth: usize,
) -> Vec<SpeechEventKind> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while seen.iter().filter(|k| **k == kind).count() < nth {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {kind:?} #{nth}: {seen:?}"
        );
        seen.extend(drain_kinds(events));
        sleep(Duration::from_millis(5)).await;
    }
    seen
}

#[test]
fn default_options_are_interruptible() {
    let o = SendTextToTtsOptions::default();
    assert!(o.interruptible);
    assert!(!o.non_blocking);
    let parsed: SendTextToTtsOptions = serde_json::from_str("{\"nonBlocking\":true}").unwrap();
    assert!(
        parsed.interruptible,
        "missing field must default to interruptible"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_job_survives_vad_barge_in_and_plays_fully() {
    let (agent, written_ms) = make_agent().await;
    let mut events = agent.subscribe_events();
    let text = "please wait while i switch to your language ".repeat(2);
    let expected_ms = mock_tts_duration_ms(&text);

    // The user bursts in only once the protected job is audibly playing, so a barge-in is
    // guaranteed to be attempted against it regardless of scheduler timing.
    let a = Arc::clone(&agent);
    let mut gate_events = agent.subscribe_events();
    tokio::spawn(async move {
        wait_for_kind(&mut gate_events, SpeechEventKind::AgentSpeakingStart, 1).await;
        user_burst(&a).await;
    });

    agent
        .send_text_to_tts_with_options(
            &text,
            SendTextToTtsOptions {
                interruptible: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();

    let played = *written_ms.lock().unwrap();
    assert!(
        played + 40 >= expected_ms,
        "protected job must play fully: played {played} ms of ~{expected_ms} ms"
    );
    let kinds = drain_kinds(&mut events);
    assert!(
        !kinds.contains(&SpeechEventKind::BargeIn),
        "suppressed barge-in must not emit barge_in: {kinds:?}"
    );
    assert!(kinds.contains(&SpeechEventKind::VadTriggered));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_queued_after_protected_job_is_flushed_by_later_barge_in() {
    let (agent, written_ms) = make_agent().await;
    let mut events = agent.subscribe_events();
    let first = "please wait a moment"; // ~1 s of mock audio
    let second = "this follow up answer is long and must be interrupted ".repeat(6);
    let first_ms = mock_tts_duration_ms(first);
    let second_ms = mock_tts_duration_ms(&second);

    agent
        .send_text_to_tts_with_options(
            first,
            SendTextToTtsOptions {
                non_blocking: true,
                interruptible: false,
            },
        )
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options(
            &second,
            SendTextToTtsOptions {
                non_blocking: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Barge-in while the protected job plays: swallowed.
    let mut seen = wait_for_kind(&mut events, SpeechEventKind::AgentSpeakingStart, 1).await;
    user_burst(&agent).await;
    // Wait for the second job's playback (next agent_speaking_start).
    seen.extend(wait_for_kind(&mut events, SpeechEventKind::AgentSpeakingStart, 1).await);
    assert!(
        !seen.contains(&SpeechEventKind::BargeIn),
        "barge-in during the protected job must be swallowed: {seen:?}"
    );

    // Second job is now playing and interruptible: a fresh barge-in flushes it.
    user_burst(&agent).await;
    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();

    let played = *written_ms.lock().unwrap();
    assert!(
        played < first_ms + second_ms * 60 / 100,
        "queued interruptible job must be cut: played {played} ms, first {first_ms}, second {second_ms}"
    );
    let kinds = drain_kinds(&mut events);
    assert!(
        kinds.contains(&SpeechEventKind::BargeIn),
        "{kinds:?} played {played}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_flush_cancels_protected_job() {
    let (agent, written_ms) = make_agent().await;
    let mut events = agent.subscribe_events();
    let text = "this protected message is long and gets cancelled by the host ".repeat(6);
    let expected_ms = mock_tts_duration_ms(&text);

    agent
        .send_text_to_tts_with_options(
            &text,
            SendTextToTtsOptions {
                non_blocking: true,
                interruptible: false,
            },
        )
        .await
        .unwrap();
    wait_for_kind(&mut events, SpeechEventKind::AgentSpeakingStart, 1).await;
    agent.flush_tts().await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();

    let played = *written_ms.lock().unwrap();
    assert!(played > 0);
    assert!(
        played < expected_ms * 60 / 100,
        "explicit flush must stop a protected job: played {played} of ~{expected_ms} ms"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_stop_cancels_protected_job() {
    let (agent, written_ms) = make_agent().await;
    let mut events = agent.subscribe_events();
    let text = "this protected message is long and gets cancelled by stop ".repeat(6);
    let expected_ms = mock_tts_duration_ms(&text);

    agent
        .send_text_to_tts_with_options(
            &text,
            SendTextToTtsOptions {
                non_blocking: true,
                interruptible: false,
            },
        )
        .await
        .unwrap();
    wait_for_kind(&mut events, SpeechEventKind::AgentSpeakingStart, 1).await;
    agent.stop().await.unwrap();

    let played = *written_ms.lock().unwrap();
    assert!(
        played < expected_ms * 60 / 100,
        "stop must end a protected job: played {played} of ~{expected_ms} ms"
    );
}
