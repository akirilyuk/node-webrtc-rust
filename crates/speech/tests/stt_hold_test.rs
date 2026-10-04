//! `begin_stt_hold` / `release_stt_hold` / `cancel_stt_hold` and `replay.maxAgeMs`.
//!
//! The recording STT keeps every PCM byte per model tag so tests can assert order (by sample
//! level), counts, and which model produced a final. Speech phases use distinct amplitudes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::stt_hold::{
    BeginSttHoldOptions, ReleaseSttHoldOptions, ReplayConfig, SttHoldMode,
};
use node_webrtc_rust_speech::{VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::broadcast::Receiver;
use tokio::time::{sleep, Duration};

type Pcm = Arc<Mutex<Vec<u8>>>;

const LEVEL_A: i16 = i16::MAX / 3; // 10922
const LEVEL_B: i16 = i16::MAX / 2; // 16383
const LEVEL_C: i16 = i16::MAX / 4; // 8191

fn stereo_frame(level: i16) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&level.to_le_bytes());
        pcm.extend_from_slice(&level.to_le_bytes());
    }
    pcm
}

struct RecordingStt {
    tag: String,
    pcm: Pcm,
    pending_final: bool,
    partial_sent: bool,
}

#[async_trait]
impl SttProvider for RecordingStt {
    fn vendor_name(&self) -> &'static str {
        "recording"
    }
    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.partial_sent = false;
        Ok(())
    }
    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
    async fn push_audio(&mut self, pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.pcm.lock().unwrap().extend_from_slice(&pcm);
        Ok(())
    }
    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        if self.pending_final {
            self.pending_final = false;
            return Ok(Some(SttTranscript::Final(format!("final-{}", self.tag))));
        }
        if !self.partial_sent && self.pcm.lock().unwrap().len() >= 1600 {
            self.partial_sent = true;
            return Ok(Some(SttTranscript::Partial(format!(
                "partial-{}",
                self.tag
            ))));
        }
        Ok(None)
    }
    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.pending_final = true;
        Ok(())
    }
}

#[derive(Default)]
struct RecordingFactory {
    by_tag: Mutex<HashMap<String, Pcm>>,
}

impl RecordingFactory {
    fn pcm(&self, tag: &str) -> Vec<u8> {
        self.by_tag
            .lock()
            .unwrap()
            .get(tag)
            .map(|p| p.lock().unwrap().clone())
            .unwrap_or_default()
    }
}

struct SharedFactory(Arc<RecordingFactory>);

impl VendorFactory for SharedFactory {
    fn create_stt(
        &self,
        config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        let tag = config.model.clone().unwrap_or_else(|| "default".into());
        let pcm = self
            .0
            .by_tag
            .lock()
            .unwrap()
            .entry(tag.clone())
            .or_default()
            .clone();
        Ok(Box::new(RecordingStt {
            tag,
            pcm,
            pending_final: false,
            partial_sent: false,
        }))
    }
    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn stt_config(model: &str, lang: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::Mock,
        model: Some(model.into()),
        model_path: None,
        language: Some(lang.into()),
        api_key: None,
        endpoint: None,
    }
}

fn agent_config(replay: ReplayConfig) -> VoiceAgentConfig {
    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.gate_stt = true;
    vad.min_silence_duration_ms = 200;
    vad.stt_gate_hold_ms = 100;
    VoiceAgentConfig {
        stt: Some(stt_config("a", "en")),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        vad,
        replay,
        ..Default::default()
    }
}

async fn start_agent(factory: &Arc<RecordingFactory>, replay: ReplayConfig) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(SharedFactory(Arc::clone(factory))),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(agent_config(replay), Arc::new(registry)).unwrap();
    let writer: node_webrtc_rust_speech::PcmWriter = Arc::new(|_p, _m| Ok(()));
    let reader: node_webrtc_rust_speech::PcmReader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

async fn speech(agent: &Arc<VoiceAgent>, level: i16, frames: usize) {
    for _ in 0..frames {
        agent
            .process_inbound_pcm(Bytes::from(stereo_frame(level)), 20)
            .await
            .unwrap();
    }
}

/// Silence long enough to close the utterance (VAD end, gate hold, endpoint tail, finalize).
async fn close_utterance(agent: &Arc<VoiceAgent>) {
    sleep(Duration::from_millis(50)).await;
    for _ in 0..80 {
        agent
            .process_inbound_pcm(Bytes::from(vec![0_u8; 3840]), 20)
            .await
            .unwrap();
    }
    sleep(Duration::from_millis(400)).await;
}

fn drain(rx: &mut Receiver<SpeechEvent>) -> Vec<SpeechEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// Sample level (mean |s|) of each 20 ms (640 byte) segment.
fn levels(pcm: &[u8]) -> Vec<u32> {
    pcm.chunks(640)
        .map(|c| {
            let n = (c.len() / 2).max(1);
            let sum: u64 = c
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() as u64)
                .sum();
            (sum / n as u64) as u32
        })
        .collect()
}

fn near(level: u32, target: i16) -> bool {
    let t = target as i64;
    (level as i64 - t).abs() < t / 5
}

fn finals(events: &[SpeechEvent]) -> Vec<&SpeechEvent> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .collect()
}

#[tokio::test]
async fn hold_open_utterance_blocks_old_stt_and_release_replays_in_order() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    let mut rx = agent.subscribe_events();

    speech(&agent, LEVEL_A, 25).await;
    let a_pcm_at_hold = factory.pcm("a");
    let a_at_hold = a_pcm_at_hold.len();
    assert!(a_at_hold > 0, "old STT must have been fed before the hold");
    let utterance_id = drain(&mut rx)
        .iter()
        .find(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .and_then(|e| e.utterance_id.clone())
        .expect("speaking start");

    agent
        .begin_stt_hold(BeginSttHoldOptions::default())
        .await
        .unwrap();
    let started = drain(&mut rx);
    let hold_started = started
        .iter()
        .find(|e| e.kind == SpeechEventKind::SttHoldStarted)
        .expect("stt_hold_started");
    assert_eq!(hold_started.hold_mode.as_deref(), Some("buffer_replay"));
    assert!(
        hold_started.buffered_ms.unwrap_or(0) > 0,
        "open utterance seeded"
    );

    // Speech continues during the hold, then the utterance closes.
    speech(&agent, LEVEL_B, 30).await;
    close_utterance(&agent).await;
    let during = drain(&mut rx);
    assert!(
        finals(&during).is_empty(),
        "no old-STT final may be emitted while held: {during:?}"
    );
    assert!(
        during
            .iter()
            .all(|e| e.kind != SpeechEventKind::UserSpeechPartial),
        "no old-STT partial while held"
    );
    assert_eq!(
        factory.pcm("a").len(),
        a_at_hold,
        "old STT receives nothing after the hold started"
    );
    assert!(
        during
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSpeakingEnd),
        "VAD/user_speaking events keep flowing"
    );

    agent
        .update_stt_config(stt_config("b", "de"))
        .await
        .unwrap();
    agent
        .release_stt_hold(ReleaseSttHoldOptions { replay: true })
        .await
        .unwrap();

    let released = drain(&mut rx);
    let replay_finals: Vec<_> = finals(&released);
    assert_eq!(replay_finals.len(), 1, "{released:?}");
    assert_eq!(replay_finals[0].text.as_deref(), Some("final-b"));
    assert_eq!(replay_finals[0].replay, Some(true));
    assert_eq!(
        replay_finals[0].replaces_utterance_id.as_deref(),
        Some(utterance_id.as_str())
    );
    let ended = released
        .iter()
        .find(|e| e.kind == SpeechEventKind::SttHoldEnded)
        .expect("stt_hold_ended");
    assert_eq!(ended.hold_outcome.as_deref(), Some("released_replay"));
    assert_eq!(ended.dropped_ms, Some(0));
    assert!(ended.buffered_ms.unwrap() >= 600);

    // New STT got the open utterance + the speech that continued, in order, no gap.
    let b = levels(&factory.pcm("b"));
    let first_b = b.iter().position(|l| near(*l, LEVEL_B)).expect("B audio");
    assert!(
        b[..first_b].iter().filter(|l| near(**l, LEVEL_A)).count() + 2
            >= levels(&a_pcm_at_hold)
                .iter()
                .filter(|l| near(**l, LEVEL_A))
                .count(),
        "open-utterance audio replayed first: {b:?}"
    );
    assert!(
        b[first_b..].iter().filter(|l| near(**l, LEVEL_A)).count() == 0,
        "A audio must not reappear after B (no duplicate/reorder)"
    );
    assert!(
        b.iter().filter(|l| near(**l, LEVEL_B)).count() >= 28,
        "all held continuing speech replayed"
    );

    // Live audio after release follows the held audio on the same STT.
    let before_live = factory.pcm("b").len();
    speech(&agent, LEVEL_C, 30).await;
    close_utterance(&agent).await;
    let live = drain(&mut rx);
    let live_finals = finals(&live);
    assert_eq!(live_finals.len(), 1, "{live:?}");
    assert_eq!(live_finals[0].text.as_deref(), Some("final-b"));
    assert_ne!(live_finals[0].replay, Some(true));
    let b_after = factory.pcm("b");
    assert!(b_after.len() > before_live);
    let tail = levels(&b_after[before_live..]);
    assert!(tail.iter().any(|l| near(*l, LEVEL_C)));
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn first_utterance_mode_keeps_triggering_utterance_only() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    let mut rx = agent.subscribe_events();

    agent
        .begin_stt_hold(BeginSttHoldOptions {
            mode: SttHoldMode::FirstUtterance,
            max_buffer_ms: None,
        })
        .await
        .unwrap();
    speech(&agent, LEVEL_A, 40).await;
    close_utterance(&agent).await;
    speech(&agent, LEVEL_B, 40).await;
    close_utterance(&agent).await;
    assert!(finals(&drain(&mut rx)).is_empty());
    assert_eq!(factory.pcm("a").len(), 0);

    agent
        .update_stt_config(stt_config("b", "de"))
        .await
        .unwrap();
    agent
        .release_stt_hold(ReleaseSttHoldOptions { replay: true })
        .await
        .unwrap();
    let released = drain(&mut rx);
    let ended = released
        .iter()
        .find(|e| e.kind == SpeechEventKind::SttHoldEnded)
        .unwrap();
    assert!(
        ended.dropped_ms.unwrap() >= 600,
        "later speech dropped: {ended:?}"
    );
    let b = levels(&factory.pcm("b"));
    assert!(b.iter().any(|l| near(*l, LEVEL_A)));
    assert!(
        b.iter().all(|l| !near(*l, LEVEL_B)),
        "second utterance must not be replayed in first_utterance mode"
    );
    assert_eq!(finals(&released).len(), 1);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn buffer_cap_drops_oldest_and_reports() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    let mut rx = agent.subscribe_events();

    agent
        .begin_stt_hold(BeginSttHoldOptions {
            mode: SttHoldMode::BufferReplay,
            max_buffer_ms: Some(200),
        })
        .await
        .unwrap();
    speech(&agent, LEVEL_A, 40).await;
    close_utterance(&agent).await;
    agent
        .update_stt_config(stt_config("b", "de"))
        .await
        .unwrap();
    agent
        .release_stt_hold(ReleaseSttHoldOptions { replay: true })
        .await
        .unwrap();
    let ev = drain(&mut rx);
    let ended = ev
        .iter()
        .find(|e| e.kind == SpeechEventKind::SttHoldEnded)
        .unwrap();
    assert!(ended.dropped_ms.unwrap() >= 500, "{ended:?}");
    assert!(ended.buffered_ms.unwrap() <= 200, "{ended:?}");
    assert!(factory.pcm("b").len() <= 200 * 32);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn release_without_replay_drops_buffer() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    let mut rx = agent.subscribe_events();
    agent
        .begin_stt_hold(BeginSttHoldOptions::default())
        .await
        .unwrap();
    speech(&agent, LEVEL_A, 40).await;
    close_utterance(&agent).await;
    agent
        .update_stt_config(stt_config("b", "de"))
        .await
        .unwrap();
    agent
        .release_stt_hold(ReleaseSttHoldOptions { replay: false })
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert!(finals(&ev).is_empty());
    assert_eq!(factory.pcm("b").len(), 0, "dropped buffer is not decoded");
    let ended = ev
        .iter()
        .find(|e| e.kind == SpeechEventKind::SttHoldEnded)
        .unwrap();
    assert_eq!(ended.hold_outcome.as_deref(), Some("released_drop"));
    assert!(agent
        .release_stt_hold(ReleaseSttHoldOptions { replay: false })
        .await
        .is_err());
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn cancel_feeds_buffered_audio_to_old_stt() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    let mut rx = agent.subscribe_events();

    speech(&agent, LEVEL_A, 25).await;
    let a_at_hold = factory.pcm("a").len();
    agent
        .begin_stt_hold(BeginSttHoldOptions::default())
        .await
        .unwrap();
    speech(&agent, LEVEL_B, 30).await;
    close_utterance(&agent).await;
    drain(&mut rx);
    assert_eq!(factory.pcm("a").len(), a_at_hold);

    agent.cancel_stt_hold().await.unwrap();
    let ev = drain(&mut rx);
    let a = levels(&factory.pcm("a"));
    assert!(
        factory.pcm("a").len() >= a_at_hold + 30 * 640,
        "old STT receives the held audio"
    );
    let first_b = a.iter().position(|l| near(*l, LEVEL_B)).unwrap();
    assert!(
        a[first_b..].iter().all(|l| !near(*l, LEVEL_A)),
        "order kept"
    );
    let f = finals(&ev);
    assert_eq!(f.len(), 1, "{ev:?}");
    assert_eq!(f[0].text.as_deref(), Some("final-a"));
    assert_ne!(f[0].replay, Some(true));
    let ended = ev
        .iter()
        .find(|e| e.kind == SpeechEventKind::SttHoldEnded)
        .unwrap();
    assert_eq!(ended.hold_outcome.as_deref(), Some("cancelled"));
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn hold_rejects_double_begin_and_replay_last() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    agent
        .begin_stt_hold(BeginSttHoldOptions::default())
        .await
        .unwrap();
    assert!(agent
        .begin_stt_hold(BeginSttHoldOptions::default())
        .await
        .is_err());
    assert!(agent.replay_last_utterance().await.is_err());
    agent.cancel_stt_hold().await.unwrap();
    assert!(agent.cancel_stt_hold().await.is_err());
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn replay_max_age_is_configurable() {
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(
        &factory,
        ReplayConfig {
            max_age_ms: Some(100),
        },
    )
    .await;
    speech(&agent, LEVEL_A, 40).await;
    close_utterance(&agent).await;
    sleep(Duration::from_millis(300)).await;
    agent
        .update_stt_config(stt_config("b", "de"))
        .await
        .unwrap();
    let err = agent.replay_last_utterance().await.unwrap_err();
    assert!(err.to_string().contains("too old"), "{err}");
    agent.stop().await.unwrap();

    // Default (10 s) still replays a utterance that is 700 ms old.
    let factory = Arc::new(RecordingFactory::default());
    let agent = start_agent(&factory, ReplayConfig::default()).await;
    speech(&agent, LEVEL_A, 40).await;
    close_utterance(&agent).await;
    sleep(Duration::from_millis(300)).await;
    agent
        .update_stt_config(stt_config("b", "de"))
        .await
        .unwrap();
    agent.replay_last_utterance().await.unwrap();
    agent.stop().await.unwrap();
}
