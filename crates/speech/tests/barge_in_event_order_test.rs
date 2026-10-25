//! `contract.event-order`: a barge-in always emits `barge_in` before the single
//! `agent_speaking_end` of that playback, whichever of the barge path and the TTS drain notices
//! the flush first.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{sleep, timeout, Duration};

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

/// Emits one qualifying partial after the first pushed audio.
struct PartialOnPollStt {
    saw_push: Mutex<bool>,
    partial_emitted: Mutex<bool>,
}

#[async_trait::async_trait]
impl SttProvider for PartialOnPollStt {
    fn vendor_name(&self) -> &'static str {
        "partial-on-poll"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        *self.saw_push.lock().unwrap() = true;
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        if !*self.saw_push.lock().unwrap() {
            return Ok(None);
        }
        let mut emitted = self.partial_emitted.lock().unwrap();
        if *emitted {
            return Ok(None);
        }
        *emitted = true;
        Ok(Some(SttTranscript::Partial("stop now".into())))
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct PartialOnPollFactory;

impl VendorFactory for PartialOnPollFactory {
    fn create_stt(
        &self,
        _config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(PartialOnPollStt {
            saw_push: Mutex::new(false),
            partial_emitted: Mutex::new(false),
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn mock_tts() -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Mock,
        model: None,
        model_path: None,
        voice: None,
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    }
}

fn vad_config(gate_stt: bool, use_vad: bool, require_stt_partial: bool) -> VadConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = use_vad;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = require_stt_partial;
    vad
}

#[derive(Clone, Copy)]
enum Scenario {
    /// STT-gated barge-in: loud frames + a qualifying partial.
    SttGated,
    /// Immediate barge-in on VAD SpeechStart (no STT).
    ImmediateVad,
    /// `flush_tts()` with barge enabled and `use_vad = false`.
    ManualFlush,
}

impl Scenario {
    fn config(self) -> VoiceAgentConfig {
        match self {
            Scenario::SttGated => VoiceAgentConfig {
                stt: Some(SttConfig {
                    provider: SttVendor::Mock,
                    model: None,
                    model_path: None,
                    language: Some("en".into()),
                    api_key: None,
                    endpoint: None,
                    transport: None,
                }),
                tts: Some(mock_tts()),
                vad: vad_config(true, true, true),
                ..Default::default()
            },
            Scenario::ImmediateVad => VoiceAgentConfig {
                stt: None,
                tts: Some(mock_tts()),
                vad: vad_config(false, true, false),
                ..Default::default()
            },
            Scenario::ManualFlush => VoiceAgentConfig {
                stt: None,
                tts: Some(mock_tts()),
                vad: vad_config(false, false, false),
                ..Default::default()
            },
        }
    }
}

/// Plays a long reply, triggers the barge once `agent_speaking_start` was seen, and returns every
/// event kind the agent emitted (including a settle window for late duplicates).
async fn run_scenario(scenario: Scenario, flush_delay: Option<Duration>) -> Vec<SpeechEventKind> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(PartialOnPollFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = VoiceAgent::new(scenario.config(), Arc::new(registry)).unwrap();
    agent.set_barge_in_flush_delay_for_test(flush_delay);
    let mut events = agent.subscribe_events();
    let seen: Arc<Mutex<Vec<SpeechEventKind>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_collector = Arc::clone(&seen);
    let collector = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            seen_collector.lock().unwrap().push(event.kind);
        }
    });

    let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(2)));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let agent = Arc::new(agent);
    let trigger_agent = Arc::clone(&agent);
    let seen_trigger = Arc::clone(&seen);
    let loud = loud_stereo_frame();
    let trigger = tokio::spawn(async move {
        // Wait (bounded) until playback has really started, then barge after 40 ms.
        let started = timeout(Duration::from_secs(5), async {
            loop {
                if seen_trigger
                    .lock()
                    .unwrap()
                    .contains(&SpeechEventKind::AgentSpeakingStart)
                {
                    return;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(
            started.is_ok(),
            "agent_speaking_start never arrived within 5 s"
        );
        sleep(Duration::from_millis(40)).await;
        match scenario {
            Scenario::SttGated | Scenario::ImmediateVad => {
                for _ in 0..8 {
                    trigger_agent
                        .process_inbound_pcm(Bytes::from(loud.clone()), 20)
                        .await
                        .unwrap();
                }
            }
            Scenario::ManualFlush => trigger_agent.flush_tts().await.unwrap(),
        }
    });

    let text = "interrupt this mock playback for the event order test ".repeat(6);
    timeout(Duration::from_secs(10), async {
        agent.send_text_to_tts(&text).await.unwrap();
        agent.wait_tts_playback_idle().await.unwrap();
    })
    .await
    .expect("playback did not finish within 10 s after the barge-in");
    timeout(Duration::from_secs(5), trigger)
        .await
        .expect("barge trigger task did not finish within 5 s")
        .expect("barge trigger task panicked");

    // Settle window: a late duplicate `agent_speaking_end` would land here.
    sleep(Duration::from_millis(300)).await;
    timeout(Duration::from_secs(5), agent.stop())
        .await
        .expect("agent.stop() did not finish within 5 s")
        .unwrap();
    collector.abort();
    let kinds = seen.lock().unwrap().clone();
    kinds
}

/// Asserts: one `agent_speaking_start`, one `barge_in`, one `agent_speaking_end` after the start,
/// and `barge_in` before that end.
fn assert_barge_before_single_end(kinds: &[SpeechEventKind], label: &str) {
    let start = kinds
        .iter()
        .position(|k| *k == SpeechEventKind::AgentSpeakingStart)
        .unwrap_or_else(|| panic!("{label}: no agent_speaking_start in {kinds:?}"));
    let barges: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, k)| **k == SpeechEventKind::BargeIn)
        .map(|(i, _)| i)
        .collect();
    let ends: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(i, k)| *i > start && **k == SpeechEventKind::AgentSpeakingEnd)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        barges.len(),
        1,
        "{label}: expected exactly one barge_in in {kinds:?}"
    );
    assert_eq!(
        ends.len(),
        1,
        "{label}: expected exactly one agent_speaking_end after agent_speaking_start in {kinds:?}"
    );
    assert!(
        barges[0] < ends[0],
        "{label}: barge_in must come before agent_speaking_end, got {kinds:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stt_gated_barge_in_precedes_agent_speaking_end_when_drain_sees_flush_first() {
    let kinds = run_scenario(Scenario::SttGated, Some(Duration::from_millis(80))).await;
    assert_barge_before_single_end(&kinds, "stt-gated, 80 ms flush delay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn immediate_vad_barge_in_precedes_agent_speaking_end() {
    let kinds = run_scenario(Scenario::ImmediateVad, Some(Duration::from_millis(80))).await;
    assert_barge_before_single_end(&kinds, "immediate VAD, 80 ms flush delay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_flush_tts_barge_emits_one_agent_speaking_end_after_barge_in() {
    let kinds = run_scenario(Scenario::ManualFlush, Some(Duration::from_millis(80))).await;
    assert_barge_before_single_end(&kinds, "manual flush_tts, 80 ms flush delay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_and_barge_never_double_emit_agent_speaking_end() {
    for run in 0..50 {
        let kinds = run_scenario(Scenario::SttGated, None).await;
        assert_barge_before_single_end(&kinds, &format!("stt-gated run {run}"));
    }
}
