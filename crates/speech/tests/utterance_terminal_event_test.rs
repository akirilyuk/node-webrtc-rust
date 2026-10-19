//! Every utterance that opened a user STT session ends with exactly one terminal event:
//! a non-empty `user_speech_final`, or `user_stt_not_found` with a `reason`
//! (`no_partial` C1, `no_transcript` stream closed with nothing, `empty_final`).
//! Never both, never neither, never an empty final.
//!
//! Repro of staging turn 40: the STT returned no partial and the vendor finalize returned no
//! final; the gate-finalize close then emitted only `user_speaking_end`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// What the scripted STT returns. `None` means the vendor says nothing for that kind.
#[derive(Clone, Copy)]
struct Script {
    /// Partial emitted once enough audio was pushed.
    partial: Option<&'static str>,
    /// Final emitted after `finalize_utterance`.
    final_on_finalize: Option<&'static str>,
}

struct ScriptedStt {
    script: Script,
    pushes: Arc<AtomicU32>,
    partial_emitted: Arc<AtomicBool>,
    finalized: Arc<AtomicBool>,
    final_emitted: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl SttProvider for ScriptedStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-terminal"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.pushes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        if let Some(partial) = self.script.partial {
            if !self.partial_emitted.load(Ordering::Relaxed)
                && self.pushes.load(Ordering::Relaxed) > 6
            {
                self.partial_emitted.store(true, Ordering::Relaxed);
                return Ok(Some(SttTranscript::Partial(partial.into())));
            }
        }
        if let Some(text) = self.script.final_on_finalize {
            if self.finalized.load(Ordering::Relaxed) && !self.final_emitted.load(Ordering::Relaxed)
            {
                self.final_emitted.store(true, Ordering::Relaxed);
                return Ok(Some(SttTranscript::Final(text.into())));
            }
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.finalized.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct ScriptedFactory {
    script: Script,
}

impl VendorFactory for ScriptedFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(ScriptedStt {
            script: self.script,
            pushes: Arc::new(AtomicU32::new(0)),
            partial_emitted: Arc::new(AtomicBool::new(false)),
            finalized: Arc::new(AtomicBool::new(false)),
            final_emitted: Arc::new(AtomicBool::new(false)),
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

struct Rig {
    agent: Arc<VoiceAgent>,
    events: tokio::sync::broadcast::Receiver<SpeechEvent>,
    seen: Vec<SpeechEvent>,
}

impl Rig {
    async fn new(script: Script, vad: VadConfig) -> Self {
        let mut registry = VendorRegistry::new();
        registry.register_stt(SttVendor::Mock, Arc::new(ScriptedFactory { script }));
        registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
        let mut vad = vad;
        vad.barge_in.enabled = false;
        let config = VoiceAgentConfig {
            stt: Some(node_webrtc_rust_speech::SttConfig {
                provider: SttVendor::Mock,
                model: None,
                model_path: None,
                language: Some("en".into()),
                api_key: None,
                endpoint: None,
                transport: None,
            }),
            tts: Some(TtsConfig {
                provider: TtsVendor::Mock,
                model: None,
                model_path: None,
                voice: None,
                api_key: None,
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            }),
            vad,
            ..Default::default()
        };
        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let events = agent.subscribe_events();
        let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
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
            // STT results are emitted by the agent's result pump; a real inbound loop is paced
            // by 20 ms frames, so let the pump catch up after every frame.
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
}

fn gate_vad() -> VadConfig {
    VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_gate_hold_ms: 80,
        ..VadConfig::default()
    }
}

/// C2 (`utterance_finalize_timeout_ms`) closes the utterance; no gate hold.
fn c2_vad() -> VadConfig {
    VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: false,
        utterance_finalize_timeout_ms: 100,
        ..VadConfig::default()
    }
}

fn count(events: &[SpeechEvent], kind: SpeechEventKind) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

fn position(events: &[SpeechEvent], kind: SpeechEventKind) -> usize {
    events
        .iter()
        .position(|e| e.kind == kind)
        .unwrap_or_else(|| panic!("missing event {kind:?}; got {:?}", kinds(events)))
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

/// Loud speech then silence past the gate hold.
async fn gate_turn(rig: &mut Rig) {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    rig.feed(&loud, 20, 20).await;
    rig.feed(&silent, 30, 20).await;
}

/// Loud speech then silence past C2 (no gate hold).
async fn c2_turn(rig: &mut Rig) {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    rig.feed(&loud, 20, 20).await;
    rig.feed(&silent, 40, 20).await;
}

async fn c1_turn(rig: &mut Rig) {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    rig.feed(&loud, 4, 20).await;
    rig.feed(&silent, 1, 5000).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    rig.feed(&silent, 2, 20).await;
}

fn c1_vad() -> VadConfig {
    VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_listen_timeout_ms: 200,
        ..VadConfig::default()
    }
}

/// Complete-pending: speech resumes after the gate hold is mostly elapsed, so the previous
/// utterance is finalized and a new one starts.
async fn complete_pending_turn(rig: &mut Rig) {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    rig.feed(&loud, 20, 20).await;
    // 14 x 20 ms = 280 ms of silence: past 90% of a 300 ms hold, still holding.
    rig.feed(&silent, 14, 20).await;
    rig.feed(&loud, 20, 20).await;
    rig.feed(&silent, 30, 20).await;
}

#[tokio::test]
async fn gate_finalize_with_no_partial_and_no_final_emits_not_found() {
    let mut rig = Rig::new(
        Script {
            partial: None,
            final_on_finalize: None,
        },
        gate_vad(),
    )
    .await;
    gate_turn(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(count(&events, SpeechEventKind::UserSttStart), 1);
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "events: {:?}",
        kinds(&events)
    );
    assert_eq!(count(&events, SpeechEventKind::UserSpeechFinal), 0);
    assert_eq!(
        not_found_reasons(&events),
        vec![Some("no_transcript".to_string())]
    );
    let stream_end = position(&events, SpeechEventKind::SttStreamEnd);
    let not_found = position(&events, SpeechEventKind::UserSttNotFound);
    let stt_end = position(&events, SpeechEventKind::UserSttEnd);
    assert!(
        stream_end < not_found && not_found < stt_end,
        "order must be stt_stream_end < user_stt_not_found < user_stt_end, got {:?}",
        kinds(&events)
    );
    assert!(
        position(&events, SpeechEventKind::UserSpeakingEnd) > stt_end,
        "user_speaking_end follows user_stt_end, got {:?}",
        kinds(&events)
    );
}

#[tokio::test]
async fn empty_vendor_final_after_vad_end_emits_not_found() {
    let mut rig = Rig::new(
        Script {
            partial: None,
            final_on_finalize: Some(""),
        },
        gate_vad(),
    )
    .await;
    gate_turn(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(count(&events, SpeechEventKind::UserSpeechFinal), 0);
    assert_eq!(
        not_found_reasons(&events),
        vec![Some("empty_final".to_string())],
        "events: {:?}",
        kinds(&events)
    );
}

#[tokio::test]
async fn forced_close_with_whitespace_partial_emits_not_found() {
    let mut rig = Rig::new(
        Script {
            partial: Some(" "),
            final_on_finalize: None,
        },
        c2_vad(),
    )
    .await;
    c2_turn(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(
        count(&events, SpeechEventKind::UserSpeechFinal),
        0,
        "events: {:?}",
        kinds(&events)
    );
    assert_eq!(
        not_found_reasons(&events),
        vec![Some("no_transcript".to_string())]
    );
}

#[tokio::test]
async fn forced_close_with_partial_emits_one_final() {
    let mut rig = Rig::new(
        Script {
            partial: Some("one two"),
            final_on_finalize: None,
        },
        c2_vad(),
    )
    .await;
    c2_turn(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(
        finals(&events),
        vec!["one two".to_string()],
        "events: {:?}",
        kinds(&events)
    );
    assert_eq!(count(&events, SpeechEventKind::UserSttNotFound), 0);
}

#[tokio::test]
async fn c1_not_found_carries_reason_no_partial() {
    let mut rig = Rig::new(
        Script {
            partial: None,
            final_on_finalize: None,
        },
        c1_vad(),
    )
    .await;
    c1_turn(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(
        not_found_reasons(&events),
        vec![Some("no_partial".to_string())],
        "events: {:?}",
        kinds(&events)
    );
    assert_eq!(count(&events, SpeechEventKind::UserSpeechFinal), 0);
}

fn complete_pending_vad() -> VadConfig {
    let mut vad = gate_vad();
    vad.stt_gate_hold_ms = 300;
    vad
}

#[tokio::test]
async fn complete_pending_with_no_output_emits_not_found_for_previous_utterance() {
    let mut rig = Rig::new(
        Script {
            partial: None,
            final_on_finalize: None,
        },
        complete_pending_vad(),
    )
    .await;
    complete_pending_turn(&mut rig).await;
    let events = rig.finish().await;

    assert_eq!(count(&events, SpeechEventKind::UserSttStart), 2);
    assert_eq!(
        not_found_reasons(&events),
        vec![
            Some("no_transcript".to_string()),
            Some("no_transcript".to_string())
        ],
        "events: {:?}",
        kinds(&events)
    );
    assert_eq!(count(&events, SpeechEventKind::UserSpeechFinal), 0);
    // Event order contract (STT actor): utterance 2's start events may precede utterance 1's
    // terminal, because the vendor answers utterance 1 asynchronously. Terminals keep utterance
    // order, and every event of an utterance carries its id.
    let not_found: Vec<&SpeechEvent> = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSttNotFound)
        .collect();
    let start_ids: Vec<Option<String>> = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .map(|e| e.utterance_id.clone())
        .collect();
    assert_eq!(start_ids.len(), 2, "events: {:?}", kinds(&events));
    assert_eq!(not_found[0].utterance_id, start_ids[0]);
    assert_eq!(not_found[1].utterance_id, start_ids[1]);
}

#[tokio::test]
async fn complete_pending_with_partial_only_emits_forced_final() {
    let mut rig = Rig::new(
        Script {
            partial: Some("one two"),
            final_on_finalize: None,
        },
        complete_pending_vad(),
    )
    .await;
    complete_pending_turn(&mut rig).await;
    let events = rig.finish().await;

    // Event order contract (STT actor): utterance 2's start events may precede utterance 1's
    // terminal. Terminals keep utterance order and carry their utterance's id.
    let first_final = position(&events, SpeechEventKind::UserSpeechFinal);
    let start_ids: Vec<Option<String>> = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .map(|e| e.utterance_id.clone())
        .collect();
    assert_eq!(start_ids.len(), 2, "events: {:?}", kinds(&events));
    assert_eq!(events[first_final].text.as_deref(), Some("one two"));
    assert_eq!(events[first_final].utterance_id, start_ids[0]);
    let second_terminal = events
        .iter()
        .position(|e| e.kind == SpeechEventKind::UserSttNotFound)
        .expect("utterance 2 ends in not_found");
    assert!(
        first_final < second_terminal,
        "utterance 1 final must precede utterance 2's terminal, got {:?}",
        kinds(&events)
    );
    assert_eq!(
        events[second_terminal].utterance_id, start_ids[1],
        "no not_found for utterance 1"
    );
}

/// One row per close path. Each row ends with exactly one terminal event per utterance and
/// never an empty final.
#[tokio::test]
async fn every_close_path_emits_exactly_one_terminal_event() {
    struct Row {
        name: &'static str,
        script: Script,
        vad: VadConfig,
        turn: u8,
    }

    let rows = vec![
        Row {
            name: "vendor final",
            script: Script {
                partial: Some("one two"),
                final_on_finalize: Some("one two"),
            },
            vad: gate_vad(),
            turn: 0,
        },
        Row {
            name: "empty vendor final",
            script: Script {
                partial: None,
                final_on_finalize: Some(""),
            },
            vad: gate_vad(),
            turn: 0,
        },
        Row {
            name: "gate finalize, no output",
            script: Script {
                partial: None,
                final_on_finalize: None,
            },
            vad: gate_vad(),
            turn: 0,
        },
        Row {
            name: "C1 no partial",
            script: Script {
                partial: None,
                final_on_finalize: None,
            },
            vad: c1_vad(),
            turn: 1,
        },
        Row {
            name: "C2 forced with partial",
            script: Script {
                partial: Some("one two"),
                final_on_finalize: None,
            },
            vad: c2_vad(),
            turn: 2,
        },
        Row {
            name: "C2 forced empty partial",
            script: Script {
                partial: Some(" "),
                final_on_finalize: None,
            },
            vad: c2_vad(),
            turn: 2,
        },
        Row {
            name: "complete-pending on new speech, no output",
            script: Script {
                partial: None,
                final_on_finalize: None,
            },
            vad: complete_pending_vad(),
            turn: 3,
        },
        Row {
            name: "complete-pending on new speech",
            script: Script {
                partial: Some("one two"),
                final_on_finalize: Some("one two"),
            },
            vad: complete_pending_vad(),
            turn: 3,
        },
    ];

    for row in rows {
        let mut rig = Rig::new(row.script, row.vad).await;
        match row.turn {
            0 => gate_turn(&mut rig).await,
            1 => c1_turn(&mut rig).await,
            2 => c2_turn(&mut rig).await,
            _ => complete_pending_turn(&mut rig).await,
        }
        let events = rig.finish().await;

        let utterances = count(&events, SpeechEventKind::UserSttStart);
        assert!(utterances >= 1, "{}: no utterance opened", row.name);
        let all_finals = finals(&events);
        let empty_finals = all_finals.iter().filter(|t| t.trim().is_empty()).count();
        let terminals =
            (all_finals.len() - empty_finals) + count(&events, SpeechEventKind::UserSttNotFound);
        assert_eq!(empty_finals, 0, "{}: empty final emitted", row.name);
        assert_eq!(
            terminals,
            utterances,
            "{}: {} utterance(s) but {} terminal event(s); events: {:?}",
            row.name,
            utterances,
            terminals,
            kinds(&events)
        );
    }
}
