//! Speaker gate: enrollment, verdict-gated barge-in, tagged finals, fail-open bounds.
//!
//! A fake `SpeakerEmbedder` returns scripted vectors (cosine decides caller / other), so the tests
//! cover the agent wiring, not the model. The Sherpa model has its own gated tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SpeakerGateConfig, SpeakerMismatchAction, SttVendor, TtsConfig, TtsVendor, VadConfig,
    VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    SpeakerEmbedder, SttProvider, SttTranscript, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::{broadcast, Notify};
use tokio::time::{sleep, Duration};

const REF: [f32; 4] = [0.9, 0.1, 0.2, 0.3];
const OTHER: [f32; 4] = [-0.2, 0.9, -0.3, 0.1];

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

// ---- fake embedder -------------------------------------------------------------------------

#[derive(Clone)]
enum Behavior {
    Vector(Vec<f32>),
    /// `embed` never settles.
    Pending,
    /// `embed` settles with the vector once the notify fires.
    WaitFor(Arc<Notify>, Vec<f32>),
}

struct FakeEmbedder {
    default: Arc<Mutex<Behavior>>,
    queue: Arc<Mutex<VecDeque<Behavior>>>,
}

#[async_trait]
impl SpeakerEmbedder for FakeEmbedder {
    async fn embed(&self, _pcm: Bytes, _rate: u32) -> SpeechResult<Option<Vec<f32>>> {
        let next = self.queue.lock().unwrap().pop_front();
        let behavior = next.unwrap_or_else(|| self.default.lock().unwrap().clone());
        match behavior {
            Behavior::Vector(v) => Ok(Some(v)),
            Behavior::Pending => std::future::pending().await,
            Behavior::WaitFor(n, v) => {
                n.notified().await;
                Ok(Some(v))
            }
        }
    }
}

#[derive(Default)]
struct Script {
    final_on_finalize: Option<String>,
}

struct ScriptedStt {
    script: Arc<Mutex<Script>>,
    queue: VecDeque<SttTranscript>,
}

#[async_trait]
impl SttProvider for ScriptedStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-speaker-gate"
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
        if let Some(text) = self.script.lock().unwrap().final_on_finalize.clone() {
            self.queue.push_back(SttTranscript::Final(text));
        }
        Ok(())
    }
}

struct GateFactory {
    script: Arc<Mutex<Script>>,
    default: Arc<Mutex<Behavior>>,
    queue: Arc<Mutex<VecDeque<Behavior>>>,
}

impl VendorFactory for GateFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(ScriptedStt {
            script: Arc::clone(&self.script),
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
        Ok(Some(Box::new(FakeEmbedder {
            default: Arc::clone(&self.default),
            queue: Arc::clone(&self.queue),
        })))
    }
}

struct Rig {
    agent: Arc<VoiceAgent>,
    script: Arc<Mutex<Script>>,
    default: Arc<Mutex<Behavior>>,
    queue: Arc<Mutex<VecDeque<Behavior>>>,
    events: broadcast::Receiver<SpeechEvent>,
    log: Vec<SpeechEvent>,
}

fn gate_cfg(on_mismatch: SpeakerMismatchAction) -> SpeakerGateConfig {
    SpeakerGateConfig {
        enabled: true,
        model_path: Some("fake.onnx".into()),
        threshold: 0.459,
        enroll_speech_ms: 400,
        check_speech_ms: 200,
        min_check_speech_ms: 100,
        on_mismatch,
        verify_timeout_ms: 300,
    }
}

fn agent_config(gate: SpeakerGateConfig) -> VoiceAgentConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_gate_hold_ms: 80,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = false;
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
        speaker_gate: gate,
        ..Default::default()
    }
}

async fn rig_with(config: VoiceAgentConfig) -> Rig {
    let script = Arc::new(Mutex::new(Script::default()));
    let default = Arc::new(Mutex::new(Behavior::Vector(REF.to_vec())));
    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::LocalSherpa,
        Arc::new(GateFactory {
            script: Arc::clone(&script),
            default: Arc::clone(&default),
            queue: Arc::clone(&queue),
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
    Rig {
        agent,
        script,
        default,
        queue,
        events,
        log: Vec::new(),
    }
}

impl Rig {
    fn drain(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            self.log.push(e);
        }
    }

    async fn feed(&self, frame: &[u8], n: usize) {
        for _ in 0..n {
            self.agent
                .process_inbound_pcm(Bytes::from(frame.to_vec()), 20)
                .await
                .unwrap();
        }
    }

    /// One utterance: `loud` frames of speech then silence long enough to close it.
    async fn utterance(&self, text: &str, loud: usize) {
        self.script.lock().unwrap().final_on_finalize = Some(text.into());
        self.feed(&loud_stereo_frame(), loud).await;
        self.feed(&silent_stereo_frame(), 30).await;
        assert!(self.agent.wait_stt_idle(Duration::from_secs(10)).await);
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

    fn finals(&self) -> Vec<&SpeechEvent> {
        self.log
            .iter()
            .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
            .collect()
    }

    fn count(&self, kind: SpeechEventKind) -> usize {
        self.log.iter().filter(|e| e.kind == kind).count()
    }

    /// Two 400 ms utterances enroll the caller (fake embedder returns `REF`).
    async fn enroll(&mut self) {
        self.utterance("one two three", 20).await;
        self.utterance("four five six", 20).await;
        for _ in 0..200 {
            if self.agent.speaker_gate_enrolled() {
                self.drain();
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("caller not enrolled");
    }

    async fn start_speaking(&self) {
        let agent = Arc::clone(&self.agent);
        let text = "interrupt this mock playback please ".repeat(8);
        tokio::spawn(async move {
            let _ = agent.send_text_to_tts(&text).await;
        });
        sleep(Duration::from_millis(200)).await;
    }
}

fn looks_like_float_list(s: &str) -> bool {
    // Two floats separated by ", " inside brackets, e.g. "[0.12, -0.3".
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'[' {
            let rest = &s[i + 1..];
            let first: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            if first.contains('.') && rest[first.len()..].starts_with(", ") {
                let after = rest[first.len() + 2..].trim_start_matches('-');
                if after.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return true;
                }
            }
        }
    }
    false
}

// ---- tests -----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_off_unchanged() {
    let mut rig = rig_with(agent_config(SpeakerGateConfig::default())).await;
    rig.utterance("hello there", 20).await;
    rig.start_speaking().await;
    rig.feed(&loud_stereo_frame(), 10).await;
    assert!(rig.wait_event(SpeechEventKind::BargeIn, 2000).await);
    rig.drain();
    assert!(rig.finals().iter().all(|f| f.speaker.is_none()));
    assert_eq!(rig.finals().len(), 1);
    rig.agent.stop().await.unwrap();
}

/// Gate config with the product defaults for the check window (`checkSpeechMs` 1500,
/// `minCheckSpeechMs` 1500); only enrollment is shortened for the test.
fn default_check_cfg() -> SpeakerGateConfig {
    SpeakerGateConfig {
        enroll_speech_ms: 400,
        verify_timeout_ms: 2000,
        ..gate_cfg(SpeakerMismatchAction::Flag)
    }
}

/// With the default `minCheckSpeechMs` (1500), an utterance with about 1.2 s of speech is not
/// checked: even though the voice differs (the embedder returns OTHER), the verdict is `unknown`
/// and the barge-in goes through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_min_check_skips_1200ms_utterance_and_barge_in_proceeds() {
    let mut cfg = default_check_cfg();
    cfg.check_speech_ms = SpeakerGateConfig::default().check_speech_ms;
    cfg.min_check_speech_ms = SpeakerGateConfig::default().min_check_speech_ms;
    let mut rig = rig_with(agent_config(cfg)).await;
    rig.enroll().await;
    *rig.default.lock().unwrap() = Behavior::Vector(OTHER.to_vec());
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("stop".into());
    // 60 frames x 20 ms = 1.2 s of speech.
    rig.feed(&loud_stereo_frame(), 60).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "unchecked short utterance fails open: barge-in proceeds"
    );
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("unknown")
    );
    rig.agent.stop().await.unwrap();
}

/// With the default config, an utterance with at least 1.5 s of speech is checked: a different
/// voice is labelled `other` and does not barge in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_min_check_checks_1800ms_utterance() {
    let mut cfg = default_check_cfg();
    cfg.check_speech_ms = SpeakerGateConfig::default().check_speech_ms;
    cfg.min_check_speech_ms = SpeakerGateConfig::default().min_check_speech_ms;
    let mut rig = rig_with(agent_config(cfg)).await;
    rig.enroll().await;
    *rig.default.lock().unwrap() = Behavior::Vector(OTHER.to_vec());
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("buy a car".into());
    // 90 frames x 20 ms = 1.8 s of speech.
    rig.feed(&loud_stereo_frame(), 90).await;
    sleep(Duration::from_millis(300)).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    rig.agent.flush_tts().await.unwrap();
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(rig.count(SpeechEventKind::BargeIn), 0);
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("other")
    );
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_enrolled_is_unknown_and_barge_in_as_today() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.start_speaking().await;
    rig.feed(&loud_stereo_frame(), 3).await;
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 2000).await,
        "no reference yet: barge-in is not deferred"
    );
    rig.script.lock().unwrap().final_on_finalize = Some("hello".into());
    rig.feed(&loud_stereo_frame(), 12).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(rig.finals()[0].speaker.as_deref(), Some("unknown"));
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enrolled_caller_barges_in_after_verdict() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    assert_eq!(
        rig.finals()[0].speaker.as_deref(),
        Some("unknown"),
        "before enrollment the verdict is unknown"
    );
    rig.start_speaking().await;
    // Fewer frames than checkSpeechMs: the verdict has not been requested yet, so no barge-in.
    rig.feed(&loud_stereo_frame(), 3).await;
    sleep(Duration::from_millis(100)).await;
    rig.drain();
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        0,
        "barge-in waits for the verdict"
    );
    rig.script.lock().unwrap().final_on_finalize = Some("let me interrupt".into());
    rig.feed(&loud_stereo_frame(), 12).await;
    assert!(rig.wait_event(SpeechEventKind::BargeIn, 3000).await);
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    let last = rig.finals().last().copied().unwrap().clone();
    assert_eq!(last.speaker.as_deref(), Some("caller"));
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bystander_is_suppressed_and_flagged() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    *rig.default.lock().unwrap() = Behavior::Vector(OTHER.to_vec());
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("buy a car".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    sleep(Duration::from_millis(300)).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    // Without a barge-in the utterance finalizes after playback; end playback now.
    rig.agent.flush_tts().await.unwrap();
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        0,
        "bystander must not barge in"
    );
    let last = rig.finals().last().copied().unwrap().clone();
    assert_eq!(last.speaker.as_deref(), Some("other"));
    assert_eq!(last.text.as_deref(), Some("buy a car"));
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bystander_with_drop_emits_ignored_and_no_final() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Drop))).await;
    rig.enroll().await;
    let finals_before = rig.finals().len();
    *rig.default.lock().unwrap() = Behavior::Vector(OTHER.to_vec());
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("buy a car".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    sleep(Duration::from_millis(300)).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    // Without a barge-in the utterance finalizes after playback; end playback now.
    rig.agent.flush_tts().await.unwrap();
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(rig.count(SpeechEventKind::BargeIn), 0);
    assert_eq!(
        rig.finals().len(),
        finals_before,
        "no final for the bystander"
    );
    let ignored: Vec<_> = rig
        .log
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeechIgnored)
        .collect();
    assert_eq!(ignored.len(), 1);
    assert_eq!(ignored[0].reason.as_deref(), Some("speaker_mismatch"));
    assert!(ignored[0].text.is_none());
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verifier_never_settles_fails_open_and_does_not_block_next() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    *rig.default.lock().unwrap() = Behavior::Pending;
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("slow verifier".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "after verifyTimeoutMs the verdict is unknown and barge-in fires"
    );
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("unknown")
    );
    assert!(
        rig.agent
            .speaker_gate_warnings()
            .iter()
            .any(|w| w.contains("speaker gate verify timeout key=")),
        "warn line expected, got {:?}",
        rig.agent.speaker_gate_warnings()
    );
    // The next utterance is not blocked by the stuck one.
    *rig.default.lock().unwrap() = Behavior::Vector(REF.to_vec());
    rig.utterance("next one", 15).await;
    rig.drain();
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("caller")
    );
    rig.agent.stop().await.unwrap();
}

/// A short enrolled-caller interruption ("stop") whose verdict settles only after the utterance
/// has ended (VAD end): the verdict still fires the barge-in while playback is active. During
/// playback the vendor finalize waits for playback, so the record is still `live` here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enrolled_short_caller_utterance_barges_in() {
    let mut cfg = gate_cfg(SpeakerMismatchAction::Flag);
    // Wall-clock test: the held verdict must outlast the wait for the vendor final.
    cfg.verify_timeout_ms = 2000;
    let mut rig = rig_with(agent_config(cfg)).await;
    rig.enroll().await;
    rig.start_speaking().await;
    let release = Arc::new(Notify::new());
    rig.queue
        .lock()
        .unwrap()
        .push_back(Behavior::WaitFor(Arc::clone(&release), REF.to_vec()));
    rig.script.lock().unwrap().final_on_finalize = Some("stop".into());
    rig.feed(&loud_stereo_frame(), 5).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    rig.drain();
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        0,
        "no barge-in before the verdict"
    );
    release.notify_waiters();
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "the held verdict fires the barge-in"
    );
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("caller")
    );
    rig.agent.stop().await.unwrap();
}

/// A short enrolled-caller utterance A whose verdict settles only after the next utterance B has
/// started (so the live record is B's): A's verdict still fires the barge-in while playback is
/// active, attributed by A's key. B's check returns `other`, so no barge-in can come from B.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_caller_verdict_fires_after_next_utterance_started() {
    let mut cfg = gate_cfg(SpeakerMismatchAction::Flag);
    // Wall-clock test: A's held verdict must outlast A's vendor final and B's start.
    cfg.verify_timeout_ms = 3000;
    let mut rig = rig_with(agent_config(cfg)).await;
    rig.enroll().await;
    let base = rig.log.len();
    // Every check after A's (B's included) says `other`.
    *rig.default.lock().unwrap() = Behavior::Vector(OTHER.to_vec());
    rig.start_speaking().await;
    let release = Arc::new(Notify::new());
    rig.queue
        .lock()
        .unwrap()
        .push_back(Behavior::WaitFor(Arc::clone(&release), REF.to_vec()));
    // Utterance A: short, then silence until its vendor final arrives.
    rig.script.lock().unwrap().final_on_finalize = Some("stop".into());
    rig.feed(&loud_stereo_frame(), 5).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    // Utterance B starts and stays live: the live record is now B's.
    rig.script.lock().unwrap().final_on_finalize = Some("bravo".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    rig.drain();
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        0,
        "no barge-in before A's verdict"
    );
    release.notify_waiters();
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "A's verdict fires the barge-in after B started"
    );
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    let finals: Vec<&SpeechEvent> = rig.log[base..]
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .collect();
    // Production order (the FIFO keeps it): A's final, then B's.
    assert_eq!(finals.len(), 2, "exactly A's and B's finals");
    assert_eq!(finals[0].speaker.as_deref(), Some("caller"), "A's final");
    assert_eq!(
        finals[1].speaker.as_deref(),
        Some("other"),
        "B keeps its own verdict"
    );
    assert_eq!(rig.count(SpeechEventKind::BargeIn), 1);
    // Production order: B's start (fed before the release) precedes the barge-in.
    let pos = |pred: &dyn Fn(&SpeechEvent) -> bool| rig.log[base..].iter().position(pred);
    let starts: Vec<usize> = rig.log[base..]
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == SpeechEventKind::UserSpeakingStart)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(starts.len(), 2, "A and B each started once");
    let barge = pos(&|e| e.kind == SpeechEventKind::BargeIn).unwrap();
    assert!(starts[1] < barge, "B started before A's verdict fired");
    rig.agent.stop().await.unwrap();
}

/// A barge-in that fires while a final waits for its verdict keeps the documented barge order
/// `agent_reply_end{interrupted}` -> `barge_in` -> `agent_speaking_end` (`contract.event-order`);
/// the reply and speaking events no longer bypass the ordered queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_reply_order_holds_while_a_final_waits() {
    let mut cfg = gate_cfg(SpeakerMismatchAction::Flag);
    cfg.verify_timeout_ms = 3000;
    let mut rig = rig_with(agent_config(cfg)).await;
    rig.enroll().await;
    let base = rig.log.len();
    let agent = Arc::clone(&rig.agent);
    let text = "interrupt this mock playback please ".repeat(8);
    tokio::spawn(async move {
        let _ = agent
            .send_text_to_tts_with_options(
                &text,
                node_webrtc_rust_speech::config::SendTextToTtsOptions {
                    non_blocking: true,
                    reply_id: Some("r1".into()),
                    ..Default::default()
                },
            )
            .await;
    });
    sleep(Duration::from_millis(200)).await;
    let release = Arc::new(Notify::new());
    rig.queue
        .lock()
        .unwrap()
        .push_back(Behavior::WaitFor(Arc::clone(&release), REF.to_vec()));
    // Short caller utterance: its final waits for the held verdict (a slot is open).
    rig.script.lock().unwrap().final_on_finalize = Some("stop".into());
    rig.feed(&loud_stereo_frame(), 5).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    rig.drain();
    assert_eq!(rig.count(SpeechEventKind::BargeIn), 0, "verdict still held");
    release.notify_waiters();
    assert!(rig.wait_event(SpeechEventKind::BargeIn, 3000).await);
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    sleep(Duration::from_millis(200)).await;
    rig.drain();
    let pos = |pred: &dyn Fn(&SpeechEvent) -> bool| rig.log[base..].iter().position(pred);
    let barge = pos(&|e| e.kind == SpeechEventKind::BargeIn).expect("barge_in");
    let reply_end =
        pos(&|e| e.kind == SpeechEventKind::AgentReplyEnd && e.reply_id.as_deref() == Some("r1"))
            .expect("agent_reply_end for r1");
    assert!(
        reply_end < barge,
        "agent_reply_end ({reply_end}) before barge_in ({barge})"
    );
    if let Some(end) = pos(&|e| e.kind == SpeechEventKind::AgentSpeakingEnd) {
        assert!(
            barge < end,
            "barge_in ({barge}) before agent_speaking_end ({end})"
        );
    }
    rig.agent.stop().await.unwrap();
}

/// Events keep production order while a final waits for its verdict (`contract.event-order`):
/// A's final (verdict pending, then `unknown` after verifyTimeoutMs) comes before every event of
/// the next utterance B, and frames never wait.
#[tokio::test(start_paused = true)]
async fn events_keep_production_order_while_a_final_waits() {
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.tts = None;
    let mut rig = rig_with(cfg).await;
    rig.enroll().await;
    let base = rig.log.len();
    // A's check pends (bounded at 300 ms); B's check, queued behind it, returns the caller.
    rig.queue.lock().unwrap().push_back(Behavior::Pending);
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    let agent = Arc::clone(&rig.agent);
    let mut worst = Duration::ZERO;
    let feed = async |frame: &[u8], n: usize, worst: &mut Duration| {
        for _ in 0..n {
            let t = tokio::time::Instant::now();
            agent
                .process_inbound_pcm(Bytes::from(frame.to_vec()), 20)
                .await
                .unwrap();
            *worst = (*worst).max(t.elapsed());
        }
    };
    rig.script.lock().unwrap().final_on_finalize = Some("alpha".into());
    feed(&loud, 15, &mut worst).await;
    feed(&silent, 30, &mut worst).await;
    sleep(Duration::from_millis(100)).await;
    rig.drain();
    let a_end = rig.log[base..]
        .iter()
        .position(|e| {
            e.kind == SpeechEventKind::SttStreamEnd && e.reason.as_deref() == Some("vendor_final")
        })
        .expect("A's vendor final arrived")
        + base;
    assert_eq!(
        rig.finals()
            .iter()
            .filter(|e| e.text.as_deref() == Some("alpha"))
            .count(),
        0
    );
    rig.script.lock().unwrap().final_on_finalize = Some("bravo".into());
    feed(&loud, 15, &mut worst).await;
    feed(&silent, 30, &mut worst).await;
    sleep(Duration::from_millis(100)).await;
    rig.drain();
    assert!(
        rig.log[a_end + 1..]
            .iter()
            .all(|e| e.kind != SpeechEventKind::UserSpeakingStart),
        "B's events wait behind A's final"
    );
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    let pos = |pred: &dyn Fn(&SpeechEvent) -> bool| {
        rig.log[a_end + 1..]
            .iter()
            .position(pred)
            .map(|p| p + a_end + 1)
    };
    let a_final =
        pos(&|e| e.kind == SpeechEventKind::UserSpeechFinal && e.text.as_deref() == Some("alpha"))
            .expect("A final");
    let b_start = pos(&|e| e.kind == SpeechEventKind::UserSpeakingStart).expect("B start");
    let b_final =
        pos(&|e| e.kind == SpeechEventKind::UserSpeechFinal && e.text.as_deref() == Some("bravo"))
            .expect("B final");
    assert!(
        a_final < b_start && b_start < b_final,
        "{a_final} {b_start} {b_final}"
    );
    assert_eq!(rig.log[a_final].speaker.as_deref(), Some("unknown"));
    assert_eq!(rig.log[b_final].speaker.as_deref(), Some("caller"));
    assert!(worst < Duration::from_millis(5), "a frame waited {worst:?}");
    rig.agent.stop().await.unwrap();
}

/// A verdict that arrives after its utterance is gone is dropped and counted
/// (`rt.stale-event-by-key`): A's pending check holds the serial verifier for its 300 ms bound,
/// B's check queues behind it, B's final hits its own bound and is emitted `unknown`, and B's
/// late verdict then finds no record.
#[tokio::test(start_paused = true)]
async fn verdict_for_finished_utterance_is_counted_stale() {
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.tts = None;
    let mut rig = rig_with(cfg).await;
    rig.enroll().await;
    {
        let mut q = rig.queue.lock().unwrap();
        q.push_back(Behavior::Pending); // A
        q.push_back(Behavior::Pending); // B: its verdict comes after B's final bound
    }
    rig.script.lock().unwrap().final_on_finalize = Some("alpha".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    sleep(Duration::from_millis(100)).await;
    rig.script.lock().unwrap().final_on_finalize = Some("bravo".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    // Past B's verdict (A's bound + B's bound) and a little more.
    sleep(Duration::from_millis(1000)).await;
    rig.drain();
    let bravo = rig
        .finals()
        .into_iter()
        .find(|e| e.text.as_deref() == Some("bravo"))
        .expect("B final");
    assert_eq!(bravo.speaker.as_deref(), Some("unknown"));
    // Both verdicts land after their utterance's record is gone: B's after B's final bound,
    // and A's after A's final (released by that very verdict) retired A's record. Neither is
    // applied; both are counted.
    assert_eq!(rig.agent.speaker_gate_stale_verdicts(), 2);
    assert_eq!(rig.count(SpeechEventKind::BargeIn), 0);
    rig.agent.stop().await.unwrap();
}

/// A's check times out (`unknown`, fail open) while B is live: A's own deferred barge-in fires
/// once on A's verdict. B's later verdict does not fire a second one, and B's final keeps B's
/// own speaker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_unknown_verdict_fires_only_its_own_barge() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    let base = rig.log.len();
    rig.start_speaking().await;
    let release = Arc::new(Notify::new());
    {
        let mut q = rig.queue.lock().unwrap();
        q.push_back(Behavior::Pending); // utterance A: times out -> late `unknown` verdict
        q.push_back(Behavior::WaitFor(Arc::clone(&release), REF.to_vec())); // utterance B
    }
    rig.script.lock().unwrap().final_on_finalize = Some("words".into());
    // A: barge-in deferred for A. Then B starts before A's verdict exists.
    rig.feed(&loud_stereo_frame(), 12).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    rig.feed(&loud_stereo_frame(), 12).await;
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "A's own unknown verdict fires A's deferred barge-in"
    );
    assert!(rig
        .agent
        .speaker_gate_warnings()
        .iter()
        .any(|w| w.contains("verify timeout")));
    release.notify_waiters();
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        1,
        "B's verdict does not fire a second barge-in"
    );
    let finals: Vec<&SpeechEvent> = rig.log[base..]
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .collect();
    assert_eq!(
        finals.last().unwrap().speaker.as_deref(),
        Some("caller"),
        "B's final keeps B's own verdict"
    );
    rig.agent.stop().await.unwrap();
}

/// Wrong-key guard: an `other` verdict for A neither suppresses nor fires B's pending barge-in;
/// B's barge-in is decided by B's verdict alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_verdict_for_old_key_leaves_next_barge_to_its_own_verdict() {
    let mut cfg = gate_cfg(SpeakerMismatchAction::Flag);
    cfg.verify_timeout_ms = 3000;
    let mut rig = rig_with(agent_config(cfg)).await;
    rig.enroll().await;
    rig.start_speaking().await;
    let release_a = Arc::new(Notify::new());
    let release_b = Arc::new(Notify::new());
    {
        let mut q = rig.queue.lock().unwrap();
        q.push_back(Behavior::WaitFor(Arc::clone(&release_a), OTHER.to_vec()));
        q.push_back(Behavior::WaitFor(Arc::clone(&release_b), REF.to_vec()));
    }
    rig.script.lock().unwrap().final_on_finalize = Some("words".into());
    rig.feed(&loud_stereo_frame(), 12).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    rig.feed(&loud_stereo_frame(), 12).await;
    release_a.notify_waiters();
    // A's `other` verdict has been applied once A's final carries it (B's check is still held).
    let mut a_applied = false;
    for _ in 0..300 {
        rig.drain();
        if rig
            .finals()
            .iter()
            .any(|e| e.speaker.as_deref() == Some("other"))
        {
            a_applied = true;
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(a_applied, "A's final carries A's other verdict");
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        0,
        "A's other verdict does not fire B's barge-in"
    );
    release_b.notify_waiters();
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "B's own verdict still fires B's barge-in (not suppressed by A's)"
    );
    rig.agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn frame_path_never_awaits_the_embedder() {
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.tts = None;
    let mut rig = rig_with(cfg).await;
    // Enrolled first: before enrollment a check is `unknown` without calling the embedder.
    rig.enroll().await;
    *rig.default.lock().unwrap() = Behavior::Pending;
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    let mut worst = Duration::ZERO;
    for i in 0..1000 {
        let frame = if (i / 25) % 2 == 0 { &loud } else { &silent };
        let t = tokio::time::Instant::now();
        rig.agent
            .process_inbound_pcm(Bytes::from(frame.clone()), 20)
            .await
            .unwrap();
        worst = worst.max(t.elapsed());
    }
    assert!(
        worst < Duration::from_millis(5),
        "a frame waited {worst:?} on the embedder"
    );
    // The checks did reach the pending embedder (each timed out off the frame path).
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    assert!(rig
        .agent
        .speaker_gate_warnings()
        .iter()
        .any(|w| w.contains("speaker gate verify timeout key=")));
    rig.agent.stop().await.unwrap();
}

/// A final that arrives before its verdict waits off the step lock: inbound frames keep
/// returning at once, and the final is emitted later with `unknown` once `verifyTimeoutMs`
/// passes (`rt.serial-queue-head-of-line`).
#[tokio::test(start_paused = true)]
async fn final_waiting_for_verdict_does_not_stall_frames() {
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.tts = None;
    let mut rig = rig_with(cfg).await;
    rig.enroll().await;
    let finals_before = rig.finals().len();
    *rig.default.lock().unwrap() = Behavior::Pending;
    rig.script.lock().unwrap().final_on_finalize = Some("final before verdict".into());
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    let mut worst = Duration::ZERO;
    let agent = Arc::clone(&rig.agent);
    let timed = |frame: &[u8]| {
        let agent = Arc::clone(&agent);
        let frame = Bytes::from(frame.to_vec());
        async move {
            let t = tokio::time::Instant::now();
            agent.process_inbound_pcm(frame, 20).await.unwrap();
            t.elapsed()
        }
    };
    for i in 0..45 {
        let frame = if i < 15 { &loud } else { &silent };
        worst = worst.max(timed(frame).await);
    }
    // Let the STT gate hold run out so the vendor final arrives; the check (sent after 200 ms
    // of speech) is still pending: its 300 ms bound has not passed.
    sleep(Duration::from_millis(100)).await;
    rig.drain();
    assert!(
        rig.log
            .iter()
            .any(|e| e.kind == SpeechEventKind::SttStreamEnd
                && e.reason.as_deref() == Some("vendor_final")),
        "the vendor final must have arrived"
    );
    assert_eq!(
        rig.finals().len(),
        finals_before,
        "the final waits for its verdict"
    );
    for _ in 0..1000 {
        worst = worst.max(timed(&silent).await);
    }
    assert!(
        worst < Duration::from_millis(5),
        "a frame waited {worst:?} behind a final waiting for its verdict"
    );
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    assert_eq!(rig.finals().len(), finals_before + 1);
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("unknown")
    );
    assert!(rig
        .agent
        .speaker_gate_warnings()
        .iter()
        .any(|w| w.contains("speaker gate verify timeout key=")));
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embeddings_are_never_logged() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    *rig.default.lock().unwrap() = Behavior::Pending;
    rig.start_speaking().await;
    rig.feed(&loud_stereo_frame(), 15).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    sleep(Duration::from_millis(500)).await;
    rig.drain();
    for w in rig.agent.speaker_gate_warnings() {
        assert!(!looks_like_float_list(&w), "float list in warn line: {w}");
    }
    for e in &rig.log {
        assert!(
            !looks_like_float_list(&format!("{e:?}")),
            "float list in event"
        );
    }
    assert!(looks_like_float_list("[0.12, -0.3]"), "detector self-check");
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_re_enrolls() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    assert!(rig.agent.speaker_gate_enrolled());
    rig.agent.reset_speaker_enrollment();
    assert!(!rig.agent.speaker_gate_enrolled());
    rig.utterance("after reset", 20).await;
    rig.drain();
    assert_eq!(
        rig.finals().last().unwrap().speaker.as_deref(),
        Some("unknown")
    );
    rig.agent.stop().await.unwrap();
}

#[test]
fn config_errors() {
    let missing = SpeakerGateConfig {
        enabled: true,
        model_path: None,
        ..SpeakerGateConfig::default()
    };
    assert!(missing.validate().unwrap_err().contains("modelPath"));
    for bad in [1.5_f32, 0.0, -0.1, 1.0] {
        let cfg = SpeakerGateConfig {
            enabled: true,
            model_path: Some("m.onnx".into()),
            threshold: bad,
            ..SpeakerGateConfig::default()
        };
        assert!(cfg.validate().unwrap_err().contains("threshold"), "{bad}");
    }
    assert!(SpeakerGateConfig::default().validate().is_ok());
    let d = SpeakerGateConfig::default();
    assert_eq!(
        (
            d.threshold,
            d.check_speech_ms,
            d.min_check_speech_ms,
            d.enroll_speech_ms,
            d.verify_timeout_ms
        ),
        (0.459, 1500, 1500, 3000, 500)
    );
    assert!(!d.enabled);
}

#[tokio::test]
async fn agent_new_rejects_bad_gate_config() {
    let registry = Arc::new(VendorRegistry::new());
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.speaker_gate.model_path = None;
    assert!(VoiceAgent::new(cfg, Arc::clone(&registry)).is_err());
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.speaker_gate.threshold = 1.5;
    assert!(VoiceAgent::new(cfg, registry).is_err());
}

/// `barge_in` names the utterance that barged in, on the immediate path (no reference yet) and
/// on the verdict path (enrolled caller), so consumers pair it by id, not by time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn barge_in_carries_utterance_id_on_immediate_and_verdict_paths() {
    fn ids(rig: &Rig) -> (Option<String>, Option<String>, Option<String>) {
        let barge = rig
            .log
            .iter()
            .rev()
            .find(|e| e.kind == SpeechEventKind::BargeIn)
            .and_then(|e| e.utterance_id.clone());
        let start = rig
            .log
            .iter()
            .rev()
            .find(|e| e.kind == SpeechEventKind::UserSpeakingStart)
            .and_then(|e| e.utterance_id.clone());
        let final_id = rig.finals().last().and_then(|e| e.utterance_id.clone());
        (barge, start, final_id)
    }

    // Immediate path: nothing enrolled, the barge fires on the first frames.
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("hello".into());
    rig.feed(&loud_stereo_frame(), 3).await;
    assert!(rig.wait_event(SpeechEventKind::BargeIn, 2000).await);
    rig.feed(&loud_stereo_frame(), 12).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    let (barge, start, final_id) = ids(&rig);
    assert!(barge.is_some(), "immediate barge_in has an utterance id");
    assert_eq!(barge, start);
    assert_eq!(barge, final_id);
    rig.agent.stop().await.unwrap();

    // Verdict path: enrolled caller, the barge fires on the verdict.
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    rig.start_speaking().await;
    rig.script.lock().unwrap().final_on_finalize = Some("let me interrupt".into());
    rig.feed(&loud_stereo_frame(), 15).await;
    assert!(rig.wait_event(SpeechEventKind::BargeIn, 3000).await);
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    let (barge, start, final_id) = ids(&rig);
    assert!(barge.is_some(), "verdict barge_in has an utterance id");
    assert_eq!(barge, start);
    assert_eq!(barge, final_id);
    rig.agent.stop().await.unwrap();
}

/// One verdict per utterance key: a second speech segment that joins the same utterance during
/// the gate hold (brief gap, same stream) neither starts a new check nor relabels the final.
/// `queued_other` stays queued when no second check runs.
async fn second_segment_on_same_key(on_mismatch: SpeakerMismatchAction) -> (Rig, usize, usize) {
    let mut rig = rig_with(agent_config(gate_cfg(on_mismatch))).await;
    rig.enroll().await;
    let finals_before = rig.finals().len();
    rig.script.lock().unwrap().final_on_finalize = Some("caller then bystander".into());
    // Caller segment: 300 ms of speech, the 200 ms check scores REF (caller).
    rig.feed(&loud_stereo_frame(), 15).await;
    sleep(Duration::from_millis(150)).await;
    // A mismatching embedding for any further check of this utterance.
    rig.queue
        .lock()
        .unwrap()
        .push_back(Behavior::Vector(OTHER.to_vec()));
    // Brief gap (SpeechEnd, then SpeechStart in the first half of the 80 ms hold): same key.
    rig.feed(&silent_stereo_frame(), 2).await;
    rig.feed(&loud_stereo_frame(), 15).await;
    sleep(Duration::from_millis(150)).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    assert!(rig.agent.wait_stt_idle(Duration::from_secs(10)).await);
    rig.drain();
    let queued = rig.queue.lock().unwrap().len();
    (rig, finals_before, queued)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_verdict_is_final_for_a_merged_utterance() {
    let (mut rig, finals_before, queued) =
        second_segment_on_same_key(SpeakerMismatchAction::Flag).await;
    assert_eq!(
        queued, 1,
        "the joined segment must not start a second check"
    );
    let finals = rig.finals();
    assert_eq!(
        finals.len(),
        finals_before + 1,
        "one final for the merged utterance"
    );
    let last = finals.last().copied().unwrap().clone();
    assert_eq!(last.text.as_deref(), Some("caller then bystander"));
    assert_eq!(last.speaker.as_deref(), Some("caller"));
    assert_eq!(rig.count(SpeechEventKind::UserSpeechIgnored), 0);
    rig.drain();
    rig.agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_verdict_is_final_with_drop() {
    let (mut rig, finals_before, queued) =
        second_segment_on_same_key(SpeakerMismatchAction::Drop).await;
    assert_eq!(
        queued, 1,
        "the joined segment must not start a second check"
    );
    let finals = rig.finals();
    assert_eq!(
        finals.len(),
        finals_before + 1,
        "a caller-labelled final is not dropped"
    );
    let last = finals.last().copied().unwrap().clone();
    assert_eq!(last.speaker.as_deref(), Some("caller"));
    assert_eq!(rig.count(SpeechEventKind::UserSpeechIgnored), 0);
    rig.drain();
    rig.agent.stop().await.unwrap();
}
