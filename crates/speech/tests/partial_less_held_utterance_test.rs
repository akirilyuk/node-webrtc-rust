//! C1 follow-up (2026-11-06): a partial-less STT vendor (`SttProvider::emits_partials() == false`,
//! e.g. Fish `transcribe-1`, Groq) and an utterance that starts while the agent plays TTS.
//!
//! The vendor answers only `finalize_utterance`, with one final. Whatever holds the utterance back
//! during playback (barge-in hold, speaker-gate hold), the utterance must end with exactly one
//! terminal event, and it is a `user_speech_final` with the vendor text.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SpeakerGateBargeIn, SpeakerGateConfig, SttVendor, TtsConfig, TtsVendor, VadConfig,
    VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    SpeakerEmbedder, SttProvider, SttTranscript, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::broadcast;
use tokio::time::{sleep, Duration};

const REF: [f32; 4] = [0.9, 0.1, 0.2, 0.3];
const HELD_TEXT: &str = "please stop talking";

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

/// Always the enrolled caller.
struct CallerEmbedder;

#[async_trait]
impl SpeakerEmbedder for CallerEmbedder {
    async fn embed(&self, _pcm: Bytes, _rate: u32) -> SpeechResult<Option<Vec<f32>>> {
        Ok(Some(REF.to_vec()))
    }
}

/// Never emits a partial; answers each `finalize_utterance` with the scripted final.
/// How the scripted vendor answers `finalize_utterance`.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    /// One final with the scripted text (empty text = an empty vendor final).
    Final,
    /// Never answers: the STT actor's finalize wait (`finalize_wait_ms`) bounds it.
    Never,
}

/// Short finalize wait so the never-answering vendor times out inside the test window.
const FINALIZE_WAIT_MS: u64 = 300;

struct FinalsOnlyStt {
    emits_partials: bool,
    answer: Answer,
    text: Arc<Mutex<String>>,
    queue: VecDeque<SttTranscript>,
}

#[async_trait]
impl SttProvider for FinalsOnlyStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-finals-only-held"
    }
    fn emits_partials(&self) -> bool {
        self.emits_partials
    }
    fn finalize_wait_ms(&self) -> u64 {
        FINALIZE_WAIT_MS
    }
    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }
    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        Ok(())
    }
    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.queue.pop_front())
    }
    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        if self.answer == Answer::Never {
            return Ok(());
        }
        let text = self.text.lock().unwrap().clone();
        self.queue.push_back(SttTranscript::Final(text));
        Ok(())
    }
}

struct Factory {
    emits_partials: bool,
    answer: Answer,
    text: Arc<Mutex<String>>,
}

impl VendorFactory for Factory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(FinalsOnlyStt {
            emits_partials: self.emits_partials,
            answer: self.answer,
            text: Arc::clone(&self.text),
            queue: VecDeque::new(),
        }))
    }
    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
    fn create_speaker_embedder(
        &self,
        _config: &SpeakerGateConfig,
    ) -> SpeechResult<Option<Box<dyn SpeakerEmbedder>>> {
        Ok(Some(Box::new(CallerEmbedder)))
    }
}

fn gate_cfg(on: bool) -> SpeakerGateConfig {
    if !on {
        return SpeakerGateConfig::default();
    }
    SpeakerGateConfig {
        enabled: true,
        model_path: Some("fake.onnx".into()),
        threshold: 0.459,
        enroll_speech_ms: 400,
        barge_in: SpeakerGateBargeIn::CallerOnly,
        ..SpeakerGateConfig::default()
    }
}

/// Product barge-in defaults (`require_stt_partial: true`), guard off so the timing is the test's.
fn agent_config(gate_on: bool) -> VoiceAgentConfig {
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
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = true;
    VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
            provider: SttVendor::LocalSherpa,
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
        speaker_gate: gate_cfg(gate_on),
        ..Default::default()
    }
}

struct Rig {
    agent: Arc<VoiceAgent>,
    text: Arc<Mutex<String>>,
    events: broadcast::Receiver<SpeechEvent>,
    log: Vec<SpeechEvent>,
}

impl Rig {
    async fn new(gate_on: bool) -> Self {
        Self::with_config(agent_config(gate_on)).await
    }

    async fn with_config(config: VoiceAgentConfig) -> Self {
        Self::with_provider(config, false).await
    }

    async fn with_provider(config: VoiceAgentConfig, emits_partials: bool) -> Self {
        Self::with_answer(config, emits_partials, Answer::Final).await
    }

    async fn with_answer(config: VoiceAgentConfig, emits_partials: bool, answer: Answer) -> Self {
        let text = Arc::new(Mutex::new(String::new()));
        let mut registry = VendorRegistry::new();
        registry.register_stt(
            SttVendor::LocalSherpa,
            Arc::new(Factory {
                emits_partials,
                answer,
                text: Arc::clone(&text),
            }),
        );
        registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
        let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
        let events = agent.subscribe_events();
        let writer: PcmWriter = Arc::new(|_pcm, _ms| {
            tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(4)));
            Ok(())
        });
        agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
        agent.start(None).await.unwrap();
        Self {
            agent,
            text,
            events,
            log: Vec::new(),
        }
    }

    fn drain(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            self.log.push(e);
        }
    }

    async fn feed(&mut self, frame: &[u8], n: usize) {
        for _ in 0..n {
            self.agent
                .process_inbound_pcm(Bytes::from(frame.to_vec()), 20)
                .await
                .unwrap();
            self.drain();
        }
    }

    async fn utterance(&mut self, text: &str, loud: usize) {
        *self.text.lock().unwrap() = text.into();
        self.feed(&loud_stereo_frame(), loud).await;
        self.feed(&silent_stereo_frame(), 30).await;
        assert!(self.agent.wait_stt_idle(Duration::from_secs(10)).await);
        self.drain();
    }

    async fn enroll(&mut self) {
        self.utterance("one two three", 20).await;
        self.utterance("four five six", 20).await;
        for _ in 0..200 {
            if self.agent.speaker_gate_enrolled() {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("caller not enrolled");
    }

    async fn wait_event(&mut self, kind: SpeechEventKind, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            self.drain();
            if self.log.iter().any(|e| e.kind == kind) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    /// Long agent reply; returns once `agent_speaking_start` was seen.
    async fn start_agent_speech(&mut self) {
        let agent = Arc::clone(&self.agent);
        let text = "the agent keeps talking while the user speaks ".repeat(8);
        tokio::spawn(async move {
            let _ = agent.send_text_to_tts(&text).await;
        });
        assert!(
            self.wait_event(SpeechEventKind::AgentSpeakingStart, 5000)
                .await,
            "agent speech never started"
        );
    }
}

/// Terminal events of the utterance with `id`: (finals with their text, not-found reasons).
fn terminals(log: &[SpeechEvent], id: &str) -> (Vec<String>, Vec<Option<String>>) {
    let mine = |e: &&SpeechEvent| e.utterance_id.as_deref() == Some(id);
    let finals = log
        .iter()
        .filter(mine)
        .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .map(|e| e.text.clone().unwrap_or_default())
        .collect();
    let not_found = log
        .iter()
        .filter(mine)
        .filter(|e| e.kind == SpeechEventKind::UserSttNotFound)
        .map(|e| e.reason.clone())
        .collect();
    (finals, not_found)
}

fn trace(log: &[SpeechEvent]) -> Vec<String> {
    log.iter()
        .map(|e| {
            format!(
                "{:?}{}{}{}",
                e.kind,
                e.utterance_id
                    .as_deref()
                    .map(|u| format!(" id={u}"))
                    .unwrap_or_default(),
                e.reason
                    .as_deref()
                    .map(|r| format!(" reason={r}"))
                    .unwrap_or_default(),
                e.text
                    .as_deref()
                    .map(|t| format!(" text={t:?}"))
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The user speaks 400 ms over the agent, then is silent; returns the utterance id.
async fn held_utterance(rig: &mut Rig) -> String {
    held_utterance_with(rig, HELD_TEXT, 20, 0).await
}

/// `loud` speech frames over the agent reply, each followed by `frame_sleep_ms` of wall time.
async fn held_utterance_with(
    rig: &mut Rig,
    text: &str,
    loud: usize,
    frame_sleep_ms: u64,
) -> String {
    rig.drain();
    let before = rig.log.len();
    rig.start_agent_speech().await;
    *rig.text.lock().unwrap() = text.into();
    for _ in 0..loud {
        rig.feed(&loud_stereo_frame(), 1).await;
        if frame_sleep_ms > 0 {
            sleep(Duration::from_millis(frame_sleep_ms)).await;
        }
    }
    rig.feed(&silent_stereo_frame(), 30).await;
    // Let playback (or the barge-in) and the vendor finalize settle.
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    // Inbound keeps flowing (silence) until the agent reply has ended, then for 1.5 s more.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !rig.log[before..]
        .iter()
        .any(|e| e.kind == SpeechEventKind::AgentSpeakingEnd)
    {
        assert!(Instant::now() < deadline, "agent reply never ended");
        rig.feed(&silent_stereo_frame(), 5).await;
        sleep(Duration::from_millis(100)).await;
    }
    for _ in 0..15 {
        rig.feed(&silent_stereo_frame(), 5).await;
        sleep(Duration::from_millis(100)).await;
    }
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    rig.log[before..]
        .iter()
        .find(|e| {
            e.kind == SpeechEventKind::VadTriggered || e.kind == SpeechEventKind::UserSttStart
        })
        .and_then(|e| e.utterance_id.clone())
        .or_else(|| {
            rig.log[before..]
                .iter()
                .find(|e| {
                    e.utterance_id.is_some()
                        && matches!(
                            e.kind,
                            SpeechEventKind::UserSpeakingStart
                                | SpeechEventKind::SttStreamStart
                                | SpeechEventKind::UserSpeechFinal
                                | SpeechEventKind::UserSttNotFound
                        )
                })
                .and_then(|e| e.utterance_id.clone())
        })
        .unwrap_or_else(|| panic!("no utterance id: {:#?}", trace(&rig.log[before..])))
}

fn assert_one_final(rig: &Rig, id: &str) {
    let (finals, not_found) = terminals(&rig.log, id);
    assert_eq!(
        (finals.clone(), not_found.clone()),
        (vec![HELD_TEXT.to_string()], vec![]),
        "exactly one terminal, a final with the vendor text; got finals={finals:?} \
         not_found={not_found:?}\n{:#?}",
        trace(&rig.log)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_less_utterance_during_playback_gate_off_gets_final() {
    let mut rig = Rig::new(false).await;
    let id = held_utterance(&mut rig).await;
    assert_one_final(&rig, &id);
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_less_utterance_during_playback_gate_caller_only_gets_final() {
    let mut rig = Rig::new(true).await;
    rig.enroll().await;
    let id = held_utterance(&mut rig).await;
    assert_one_final(&rig, &id);
    rig.agent.stop().await.unwrap();
}

/// Barge-in suppressed by the playback guard: the agent keeps playing over the whole utterance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_less_utterance_inside_playback_guard_gets_final() {
    let mut config = agent_config(false);
    config.vad.barge_in.agent_playback_guard_ms = 60_000;
    let mut rig = Rig::with_config(config).await;
    let id = held_utterance(&mut rig).await;
    assert_one_final(&rig, &id);
    rig.agent.stop().await.unwrap();
}

/// Barge-in off: the agent keeps playing over the whole utterance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_less_utterance_barge_off_during_playback_gets_final() {
    let mut config = agent_config(false);
    config.vad.barge_in.enabled = false;
    let mut rig = Rig::with_config(config).await;
    let id = held_utterance(&mut rig).await;
    assert_one_final(&rig, &id);
    rig.agent.stop().await.unwrap();
}

/// Product STT listen timeout (4000 ms) and default guard, gate on: the realistic shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_less_utterance_product_defaults_gate_on_gets_final() {
    let mut config = agent_config(true);
    config.vad.stt_listen_timeout_ms = VadConfig::default().stt_listen_timeout_ms;
    config.vad.barge_in.agent_playback_guard_ms =
        VadConfig::default().barge_in.agent_playback_guard_ms;
    let mut rig = Rig::with_config(config).await;
    rig.enroll().await;
    let id = held_utterance(&mut rig).await;
    assert_one_final(&rig, &id);
    rig.agent.stop().await.unwrap();
}

/// Partial-capable provider that sends no partial for this phrase (a short word), barge-in off,
/// playback longer than `stt_listen_timeout_ms`: C1 runs on the wall clock while the close
/// (and so the vendor final) is blocked by `agent_speaking`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_partial_phrase_held_past_listen_timeout_during_playback_gets_final() {
    let mut config = agent_config(false);
    config.vad.barge_in.enabled = false;
    config.vad.stt_listen_timeout_ms = 1000;
    let mut rig = Rig::with_provider(config, true).await;
    let id = held_utterance(&mut rig).await;
    assert_one_final(&rig, &id);
    rig.agent.stop().await.unwrap();
}

fn no_partial_held_config() -> VoiceAgentConfig {
    let mut config = agent_config(false);
    config.vad.barge_in.enabled = false;
    config.vad.stt_listen_timeout_ms = 1000;
    config
}

fn stream_end_reasons(log: &[SpeechEvent], id: &str) -> Vec<Option<String>> {
    log.iter()
        .filter(|e| {
            e.kind == SpeechEventKind::SttStreamEnd && e.utterance_id.as_deref() == Some(id)
        })
        .map(|e| e.reason.clone())
        .collect()
}

/// Held past the listen timeout, the vendor answers the finalize with an empty final: one
/// `user_stt_not_found{empty_final}` from the close (the product reason for an empty vendor
/// final), not C1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_partial_phrase_held_empty_vendor_final_is_empty_final() {
    let mut rig = Rig::with_provider(no_partial_held_config(), true).await;
    let id = held_utterance_with(&mut rig, "", 20, 0).await;
    let (finals, not_found) = terminals(&rig.log, &id);
    assert_eq!(
        (finals, not_found),
        (vec![], vec![Some("empty_final".to_string())]),
        "{:#?}",
        trace(&rig.log)
    );
    assert!(
        !stream_end_reasons(&rig.log, &id).contains(&Some("listen_timeout".to_string())),
        "{:#?}",
        trace(&rig.log)
    );
    rig.agent.stop().await.unwrap();
}

/// VAD still hears speech past the listen timeout during playback: C1 runs as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_partial_speech_past_listen_timeout_during_playback_c1_fires() {
    let mut rig = Rig::with_provider(no_partial_held_config(), true).await;
    // 80 x 20 ms frames over >= 1.6 s of wall time: media and wall clock both pass 1000 ms.
    let id = held_utterance_with(&mut rig, HELD_TEXT, 80, 20).await;
    let (finals, not_found) = terminals(&rig.log, &id);
    assert_eq!(
        (finals, not_found),
        (vec![], vec![Some("no_partial".to_string())]),
        "{:#?}",
        trace(&rig.log)
    );
    assert!(stream_end_reasons(&rig.log, &id).contains(&Some("listen_timeout".to_string())));
    rig.agent.stop().await.unwrap();
}

/// After playback ends the close sends the finalize; a vendor that never answers is bounded by
/// the STT actor's finalize wait (`finalize_wait_ms`, here FINALIZE_WAIT_MS), not by a new timer:
/// exactly one `user_stt_not_found`, inside the 1.5 s the test feeds after playback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_partial_phrase_held_vendor_never_answers_is_bounded() {
    let mut rig = Rig::with_answer(no_partial_held_config(), true, Answer::Never).await;
    let id = held_utterance(&mut rig).await;
    let (finals, not_found) = terminals(&rig.log, &id);
    assert!(finals.is_empty(), "{:#?}", trace(&rig.log));
    assert_eq!(not_found.len(), 1, "{:#?}", trace(&rig.log));
    assert_ne!(
        not_found,
        vec![Some("no_partial".to_string())],
        "{:#?}",
        trace(&rig.log)
    );
    rig.agent.stop().await.unwrap();
}
