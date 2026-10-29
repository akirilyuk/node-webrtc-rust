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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_verdict_of_old_key_is_not_applied_to_next_utterance() {
    let mut rig = rig_with(agent_config(gate_cfg(SpeakerMismatchAction::Flag))).await;
    rig.enroll().await;
    rig.start_speaking().await;
    let release = Arc::new(Notify::new());
    {
        let mut q = rig.queue.lock().unwrap();
        q.push_back(Behavior::Pending); // utterance A: times out -> late `unknown` verdict
        q.push_back(Behavior::WaitFor(Arc::clone(&release), REF.to_vec())); // utterance B
    }
    // A: barge-in deferred for A. Then B starts before A's verdict exists.
    rig.script.lock().unwrap().final_on_finalize = None;
    rig.feed(&loud_stereo_frame(), 12).await;
    rig.feed(&silent_stereo_frame(), 30).await;
    rig.feed(&loud_stereo_frame(), 12).await;
    // A's verdict (timeout, unknown) arrives while B is live.
    for _ in 0..300 {
        if rig
            .agent
            .speaker_gate_warnings()
            .iter()
            .any(|w| w.contains("verify timeout"))
        {
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    sleep(Duration::from_millis(100)).await;
    rig.drain();
    assert_eq!(
        rig.count(SpeechEventKind::BargeIn),
        0,
        "A's late verdict must not fire B's barge-in"
    );
    release.notify_waiters();
    assert!(
        rig.wait_event(SpeechEventKind::BargeIn, 3000).await,
        "B's own verdict fires the barge-in"
    );
    rig.agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn frame_path_never_awaits_the_embedder() {
    let mut cfg = agent_config(gate_cfg(SpeakerMismatchAction::Flag));
    cfg.tts = None;
    let rig = rig_with(cfg).await;
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
        (0.459, 1500, 1000, 3000, 500)
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
