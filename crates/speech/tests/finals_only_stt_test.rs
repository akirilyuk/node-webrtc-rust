//! Finals-only STT vendors (`SttProvider::emits_partials() == false`, e.g. Fish `transcribe-1`,
//! Groq, Azure): the answer comes only after `finalize_utterance`.
//!
//! Staging 2026-10-17: C1 (`stt_listen_timeout_ms`, no partial after `vad_triggered`) closed a
//! long utterance with `user_stt_not_found` / `stt_stream_end{listen_timeout}` before the vendor
//! answered its commit, and the final was dropped. Barge-in with `require_stt_partial` never
//! fired for such a vendor either, because no partial ever arrives.
//!
//! The scaled timeouts here (`stt_listen_timeout_ms` 200 instead of 4000) keep the tests fast; a
//! "6 s utterance" is fed as 500 ms frames, so the audio clock passes the timeout at once and the
//! wall clock passes it during the explicit sleep.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

const COUNTING: &str = "one two three four five six seven eight nine ten";

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn silent_stereo_frame() -> Vec<u8> {
    vec![0_u8; 3840]
}

/// Never emits a partial. Answers `finalize_utterance` with `final_text` (nothing when `None`).
struct ScriptedStt {
    emits_partials: bool,
    final_text: Option<&'static str>,
    finalized: bool,
    final_sent: bool,
}

#[async_trait::async_trait]
impl SttProvider for ScriptedStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-finals-only"
    }

    fn emits_partials(&self) -> bool {
        self.emits_partials
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        if self.finalized && !self.final_sent {
            if let Some(text) = self.final_text {
                self.final_sent = true;
                return Ok(Some(SttTranscript::Final(text.into())));
            }
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.finalized = true;
        self.final_sent = false;
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

/// `SttConfig.model == "finals-only"` selects a finals-only provider, anything else a
/// partial-capable one (that happens to say nothing). Lets `update_stt_config` swap between them.
struct ModeFactory {
    final_text: Option<&'static str>,
    /// `emits_partials` of the provider created for a config without the `finals-only` model.
    default_emits_partials: bool,
}

impl VendorFactory for ModeFactory {
    fn create_stt(
        &self,
        config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        let finals_only = config.model.as_deref() == Some("finals-only");
        Ok(Box::new(ScriptedStt {
            emits_partials: !finals_only && self.default_emits_partials,
            final_text: self.final_text,
            finalized: false,
            final_sent: false,
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn stt_config(model: Option<&str>) -> node_webrtc_rust_speech::SttConfig {
    node_webrtc_rust_speech::SttConfig {
        provider: SttVendor::Mock,
        model: model.map(str::to_string),
        model_path: None,
        language: Some("en".into()),
        api_key: None,
        endpoint: None,
    }
}

fn tts_config() -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Mock,
        model: None,
        model_path: None,
        voice: None,
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
    }
}

fn gate_vad() -> VadConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_gate_hold_ms: 80,
        stt_listen_timeout_ms: 200,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = false;
    vad
}

fn barge_vad(require_stt_partial: bool) -> VadConfig {
    let mut vad = gate_vad();
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = require_stt_partial;
    vad
}

struct Rig {
    agent: Arc<VoiceAgent>,
    events: tokio::sync::broadcast::Receiver<SpeechEvent>,
    seen: Vec<SpeechEvent>,
}

impl Rig {
    async fn new(factory: ModeFactory, vad: VadConfig, model: Option<&str>) -> Self {
        let mut registry = VendorRegistry::new();
        registry.register_stt(SttVendor::Mock, Arc::new(factory));
        registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
        let config = VoiceAgentConfig {
            stt: Some(stt_config(model)),
            tts: Some(tts_config()),
            vad,
            ..Default::default()
        };
        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let events = agent.subscribe_events();
        // Playback paces itself on the writer, so agent speech stays audible for a while.
        let writer: PcmWriter = Arc::new(|_pcm, _ms| {
            std::thread::sleep(Duration::from_millis(4));
            Ok(())
        });
        agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
        agent.start(None).await.unwrap();
        Self {
            agent,
            events,
            seen: Vec::new(),
        }
    }

    async fn feed(&mut self, frame: &[u8], frames: usize, duration_ms: u32) {
        for _ in 0..frames {
            self.agent
                .process_inbound_pcm(Bytes::from(frame.to_vec()), duration_ms)
                .await
                .unwrap();
            assert!(self.agent.wait_stt_idle(Duration::from_secs(10)).await);
            self.drain();
        }
    }

    fn drain(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.seen.push(event);
        }
    }

    async fn finish(mut self) -> Vec<SpeechEvent> {
        assert!(self.agent.wait_stt_idle(Duration::from_secs(10)).await);
        self.agent.stop().await.unwrap();
        self.drain();
        self.seen
    }

    /// Starts a long agent reply and returns once `agent_speaking_start` was emitted.
    async fn start_agent_speech(&mut self) {
        let agent = Arc::clone(&self.agent);
        let text = "the agent keeps talking while the user speaks ".repeat(6);
        tokio::spawn(async move {
            let _ = agent.send_text_to_tts(&text).await;
        });
        let started = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.events.recv().await {
                    Ok(event) => {
                        let is_start = event.kind == SpeechEventKind::AgentSpeakingStart;
                        self.seen.push(event);
                        if is_start {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => panic!("event channel closed"),
                }
            }
        })
        .await;
        assert!(started.is_ok(), "agent speech never started");
    }
}

/// A 6 s utterance: 12 loud 500 ms frames, a wall-clock pause past the (scaled) C1 timeout in
/// the middle, then silence past the gate hold.
async fn long_utterance(rig: &mut Rig) {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    rig.feed(&loud, 6, 500).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    rig.feed(&loud, 6, 500).await;
    rig.feed(&silent, 30, 20).await;
}

fn count(events: &[SpeechEvent], kind: SpeechEventKind) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

fn kinds(events: &[SpeechEvent]) -> Vec<SpeechEventKind> {
    events.iter().map(|e| e.kind).collect()
}

fn finals(events: &[SpeechEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .map(|e| e.text.clone().unwrap_or_default())
        .collect()
}

fn not_found_reasons(events: &[SpeechEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSttNotFound)
        .map(|e| e.reason.clone())
        .collect()
}

fn stream_end_reasons(events: &[SpeechEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::SttStreamEnd)
        .map(|e| e.reason.clone())
        .collect()
}

#[tokio::test]
async fn finals_only_stt_long_utterance_gets_final() {
    let mut rig = Rig::new(
        ModeFactory {
            final_text: Some(COUNTING),
            default_emits_partials: false,
        },
        gate_vad(),
        None,
    )
    .await;
    long_utterance(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(
        finals(&events),
        vec![COUNTING.to_string()],
        "{:?}",
        kinds(&events)
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        0,
        "{:?}",
        kinds(&events)
    );
    assert!(
        !stream_end_reasons(&events).contains(&Some("listen_timeout".to_string())),
        "C1 must not close a finals-only utterance: {:?}",
        stream_end_reasons(&events)
    );
}

#[tokio::test]
async fn finals_only_stt_empty_finalize_is_not_found() {
    let mut rig = Rig::new(
        ModeFactory {
            final_text: None,
            default_emits_partials: false,
        },
        gate_vad(),
        None,
    )
    .await;
    long_utterance(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "the utterance still ends with a terminal event: {:?}",
        kinds(&events)
    );
    assert_eq!(count(&events, SpeechEventKind::UserSpeechFinal), 0);
    assert!(
        !stream_end_reasons(&events).contains(&Some("listen_timeout".to_string())),
        "the terminal comes from the finalize path, not C1: {:?}",
        stream_end_reasons(&events)
    );
    assert_ne!(
        not_found_reasons(&events),
        vec![Some("no_partial".to_string())],
        "no_partial is the C1 reason"
    );
}

#[tokio::test]
async fn partial_stt_c1_still_fires() {
    let mut rig = Rig::new(
        ModeFactory {
            final_text: Some(COUNTING),
            default_emits_partials: true,
        },
        gate_vad(),
        None,
    )
    .await;
    long_utterance(&mut rig).await;
    let events = rig.finish().await;

    assert!(
        stream_end_reasons(&events).contains(&Some("listen_timeout".to_string())),
        "a partial-capable provider with no partial keeps the C1 cut-off: {:?}",
        stream_end_reasons(&events)
    );
    assert_eq!(
        not_found_reasons(&events),
        vec![Some("no_partial".to_string())],
        "{:?}",
        kinds(&events)
    );
}

/// Agent reply playing, then user speech: whether VAD alone barges in.
async fn barge_scenario(rig: &mut Rig) -> bool {
    rig.start_agent_speech().await;
    let loud = loud_stereo_frame();
    rig.feed(&loud, 8, 20).await;
    count(&rig.seen, SpeechEventKind::BargeIn) > 0
}

#[tokio::test]
async fn finals_only_stt_vad_barges_in_with_require_partial() {
    let mut rig = Rig::new(
        ModeFactory {
            final_text: Some(COUNTING),
            default_emits_partials: false,
        },
        barge_vad(true),
        None,
    )
    .await;
    assert!(
        barge_scenario(&mut rig).await,
        "a finals-only provider never sends the partial the gate waits for: VAD must barge in"
    );
    rig.finish().await;
}

#[tokio::test]
async fn partial_capable_stt_without_partial_does_not_barge_in() {
    let mut rig = Rig::new(
        ModeFactory {
            final_text: Some(COUNTING),
            default_emits_partials: true,
        },
        barge_vad(true),
        None,
    )
    .await;
    assert!(
        !barge_scenario(&mut rig).await,
        "partial-capable provider: barge-in still waits for a qualifying partial"
    );
    rig.finish().await;
}

#[tokio::test]
async fn update_stt_to_finals_only_enables_vad_barge_in() {
    let mut rig = Rig::new(
        ModeFactory {
            final_text: Some(COUNTING),
            default_emits_partials: true,
        },
        barge_vad(true),
        None,
    )
    .await;
    rig.agent
        .update_stt_config(stt_config(Some("finals-only")))
        .await
        .unwrap();
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    assert!(
        barge_scenario(&mut rig).await,
        "after update_stt to a finals-only provider, VAD barge-in must fire"
    );
    rig.finish().await;
}
