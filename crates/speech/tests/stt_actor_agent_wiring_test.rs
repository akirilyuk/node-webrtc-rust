//! VoiceAgent wiring of the per-agent STT actor: every utterance event carries its record's id,
//! the inbound path never waits for the STT vendor, and a language-id outcome that arrives after
//! its utterance closed is still written to that utterance.
//!
//! The scripted providers are driven by gates (`AtomicBool` / `Notify`), not by sleeps.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    LanguageIdConfig, SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    FinalizeMode, LanguageIdProvider, LanguageIdResult, SttOutput, SttProvider, SttTranscript,
    TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::{broadcast, Notify};
use tokio::time::timeout;

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

/// Inline provider: one partial after enough audio (when scripted), never a final.
struct InlineStt {
    partial: Option<&'static str>,
    pushes: usize,
    partial_sent: bool,
}

#[async_trait]
impl SttProvider for InlineStt {
    fn vendor_name(&self) -> &'static str {
        "wiring-inline"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        self.pushes += 1;
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        if let Some(text) = self.partial {
            if !self.partial_sent && self.pushes > 6 {
                self.partial_sent = true;
                return Ok(Some(SttTranscript::Partial(text.into())));
            }
        }
        Ok(None)
    }
}

/// Shared gates of [`GatedDeferredStt`].
#[derive(Default)]
struct Gates {
    /// While false the provider keeps the vendor's answer to every finalize back.
    release: AtomicBool,
    finalize_requests: AtomicUsize,
}

/// Deferred-finalize provider: `request_finalize` returns at once, and the final plus the
/// `FinalizeDone` marker only come out of `poll_output` once the test released the gate.
struct GatedDeferredStt {
    gates: Arc<Gates>,
    pending: usize,
    out: VecDeque<SttOutput>,
    pushes: usize,
}

#[async_trait]
impl SttProvider for GatedDeferredStt {
    fn vendor_name(&self) -> &'static str {
        "wiring-gated-deferred"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        self.pushes += 1;
        if self.pushes == 6 {
            self.out
                .push_back(SttOutput::Transcript(SttTranscript::Partial(
                    "hello".into(),
                )));
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }

    fn finalize_mode(&self) -> FinalizeMode {
        FinalizeMode::Deferred
    }

    async fn request_finalize(&mut self) -> SpeechResult<()> {
        self.pending += 1;
        self.gates.finalize_requests.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        if self.pending > 0 && self.gates.release.load(Ordering::SeqCst) {
            self.pending -= 1;
            self.out
                .push_back(SttOutput::Transcript(SttTranscript::Final(
                    "hello world".into(),
                )));
            self.out.push_back(SttOutput::FinalizeDone);
        }
        Ok(self.out.pop_front())
    }

    fn finalize_wait_ms(&self) -> u64 {
        60_000
    }
}

/// Inline provider whose `finalize_utterance` does not return until the test releases it.
struct BlockingFinalizeStt {
    release: Arc<Notify>,
}

#[async_trait]
impl SttProvider for BlockingFinalizeStt {
    fn vendor_name(&self) -> &'static str {
        "wiring-blocking-finalize"
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
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        self.release.notified().await;
        Ok(())
    }

    fn finalize_wait_ms(&self) -> u64 {
        60_000
    }
}

/// Language ID that answers `de` only after the test released it.
struct GatedLanguageId {
    release: Arc<Notify>,
}

#[async_trait]
impl LanguageIdProvider for GatedLanguageId {
    async fn identify(
        &self,
        _pcm: Bytes,
        _sample_rate: u32,
    ) -> SpeechResult<Option<LanguageIdResult>> {
        self.release.notified().await;
        Ok(Some(LanguageIdResult {
            language: "de".into(),
        }))
    }
}

type SttBuilder = Box<dyn Fn() -> Box<dyn SttProvider> + Send + Sync>;

struct WiringFactory {
    stt: SttBuilder,
    lid_release: Option<Arc<Notify>>,
}

impl VendorFactory for WiringFactory {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok((self.stt)())
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        MockFactory.create_tts(config)
    }

    fn create_language_id(
        &self,
        _config: &LanguageIdConfig,
    ) -> SpeechResult<Option<Box<dyn LanguageIdProvider>>> {
        Ok(self.lid_release.as_ref().map(|release| {
            Box::new(GatedLanguageId {
                release: Arc::clone(release),
            }) as Box<dyn LanguageIdProvider>
        }))
    }
}

async fn agent_with(
    stt: SttBuilder,
    vad: VadConfig,
    language_id: Option<(LanguageIdConfig, Arc<Notify>)>,
) -> Arc<VoiceAgent> {
    let (language_id_config, lid_release) = match language_id {
        Some((config, release)) => (Some(config), Some(release)),
        None => (None, None),
    };
    let factory = Arc::new(WiringFactory { stt, lid_release });
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::clone(&factory) as Arc<dyn VendorFactory>,
    );
    registry.register_stt(SttVendor::LocalSherpa, factory);
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let mut vad = vad;
    vad.barge_in.enabled = false;
    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
        }),
        language_id: language_id_config,
        vad,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
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

fn c1_vad() -> VadConfig {
    VadConfig {
        stt_listen_timeout_ms: 200,
        ..gate_vad()
    }
}

fn c2_vad() -> VadConfig {
    VadConfig {
        gate_stt: false,
        utterance_finalize_timeout_ms: 100,
        ..gate_vad()
    }
}

async fn feed(agent: &Arc<VoiceAgent>, frame: &[u8], frames: usize, duration_ms: u32) {
    for _ in 0..frames {
        agent
            .process_inbound_pcm(Bytes::from(frame.to_vec()), duration_ms)
            .await
            .unwrap();
        // A real inbound loop is paced by 20 ms frames: let the result pump catch up.
        assert!(agent.wait_stt_idle(Duration::from_secs(10)).await);
    }
}

fn drain(rx: &mut broadcast::Receiver<SpeechEvent>) -> Vec<SpeechEvent> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

/// Every `user_speaking_end` has the id of the `user_speaking_start` of its utterance (the
/// n-th end belongs to the n-th start), and every utterance event of the STT lifecycle has an id.
fn assert_ids_everywhere(events: &[SpeechEvent], expected_utterances: usize) {
    let starts: Vec<&SpeechEvent> = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .collect();
    let ends: Vec<&SpeechEvent> = events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeakingEnd)
        .collect();
    assert_eq!(starts.len(), expected_utterances, "starts: {events:?}");
    assert_eq!(ends.len(), expected_utterances, "ends: {events:?}");
    for (start, end) in starts.iter().zip(ends.iter()) {
        assert!(start.utterance_id.is_some(), "start without id");
        assert_eq!(
            end.utterance_id, start.utterance_id,
            "user_speaking_end must carry its utterance's id: {events:?}"
        );
    }
    for event in events {
        if matches!(
            event.kind,
            SpeechEventKind::UserSttStart
                | SpeechEventKind::SttStreamStart
                | SpeechEventKind::SttStreamEnd
                | SpeechEventKind::UserSttEnd
                | SpeechEventKind::UserSttNotFound
                | SpeechEventKind::UserSpeechPartial
                | SpeechEventKind::UserSpeechFinal
        ) {
            assert!(
                event.utterance_id.is_some(),
                "{:?} must carry the utterance id: {events:?}",
                event.kind
            );
        }
    }
}

#[tokio::test]
async fn speaking_end_always_carries_utterance_id() {
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // Gate close: no partial, no final -> not_found at the gate finalize.
    let agent = agent_with(
        Box::new(|| {
            Box::new(InlineStt {
                partial: None,
                pushes: 0,
                partial_sent: false,
            })
        }),
        gate_vad(),
        None,
    )
    .await;
    let mut rx = agent.subscribe_events();
    feed(&agent, &loud, 20, 20).await;
    feed(&agent, &silent, 30, 20).await;
    assert_ids_everywhere(&drain(&mut rx), 1);
    agent.stop().await.unwrap();

    // C1: no partial within the listen timeout; VAD stops hearing speech afterwards.
    let agent = agent_with(
        Box::new(|| {
            Box::new(InlineStt {
                partial: None,
                pushes: 0,
                partial_sent: false,
            })
        }),
        c1_vad(),
        None,
    )
    .await;
    let mut rx = agent.subscribe_events();
    feed(&agent, &loud, 4, 20).await;
    feed(&agent, &silent, 1, 5000).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    feed(&agent, &silent, 2, 20).await;
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSttNotFound
                && e.reason.as_deref() == Some("no_partial")),
        "C1 path not taken: {events:?}"
    );
    assert_ids_everywhere(&events, 1);
    agent.stop().await.unwrap();

    // C2: partial but no final; the end-of-turn timer forces the close.
    let agent = agent_with(
        Box::new(|| {
            Box::new(InlineStt {
                partial: Some("one two"),
                pushes: 0,
                partial_sent: false,
            })
        }),
        c2_vad(),
        None,
    )
    .await;
    let mut rx = agent.subscribe_events();
    feed(&agent, &loud, 20, 20).await;
    feed(&agent, &silent, 40, 20).await;
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSpeechFinal),
        "C2 path not taken: {events:?}"
    );
    assert_ids_everywhere(&events, 1);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn inbound_returns_while_finalize_is_pending() {
    let gates = Arc::new(Gates::default());
    let gates_for_provider = Arc::clone(&gates);
    let agent = agent_with(
        Box::new(move || {
            Box::new(GatedDeferredStt {
                gates: Arc::clone(&gates_for_provider),
                pending: 0,
                out: VecDeque::new(),
                pushes: 0,
            })
        }),
        gate_vad(),
        None,
    )
    .await;
    let mut rx = agent.subscribe_events();
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    feed(&agent, &loud, 20, 20).await;
    // Silence past the gate hold: the utterance is closed, its FinalizeDone is gated. (No
    // `wait_stt_idle` here: the barrier would wait for the gated finalize.)
    for _ in 0..30 {
        timeout(
            Duration::from_secs(1),
            agent.process_inbound_pcm(Bytes::from(silent.clone()), 20),
        )
        .await
        .expect("process_inbound_pcm waited for the pending finalize")
        .unwrap();
    }
    // The actor handled the finalize request (its barrier is parked behind it).
    timeout(Duration::from_secs(5), async {
        while gates.finalize_requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("finalize requested");
    assert_eq!(gates.finalize_requests.load(Ordering::SeqCst), 1);

    // 200 frames of the next turn while the vendor still owes the first final: none of them
    // may wait for it.
    for i in 0..200 {
        let frame = if i % 2 == 0 { &loud } else { &silent };
        timeout(
            Duration::from_secs(1),
            agent.process_inbound_pcm(Bytes::from(frame.clone()), 20),
        )
        .await
        .expect("process_inbound_pcm waited for the pending finalize")
        .unwrap();
    }
    let before_release = drain(&mut rx);
    assert!(
        !before_release
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSpeechFinal),
        "the gated final must not be emitted yet: {before_release:?}"
    );

    // The vendor answers: the first utterance ends with its final, with its own id.
    gates.release.store(true, Ordering::SeqCst);
    assert!(agent.wait_stt_idle(Duration::from_secs(10)).await);
    let events = drain(&mut rx);
    let first_start = before_release
        .iter()
        .find(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .and_then(|e| e.utterance_id.clone())
        .expect("first utterance started");
    let first_final = events
        .iter()
        .find(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .expect("final after release");
    assert_eq!(first_final.text.as_deref(), Some("hello world"));
    assert_eq!(
        first_final.utterance_id.as_deref(),
        Some(first_start.as_str())
    );
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn inbound_returns_while_inline_finalize_blocks() {
    let release = Arc::new(Notify::new());
    let release_for_provider = Arc::clone(&release);
    let agent = agent_with(
        Box::new(move || {
            Box::new(BlockingFinalizeStt {
                release: Arc::clone(&release_for_provider),
            })
        }),
        gate_vad(),
        None,
    )
    .await;
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    feed(&agent, &loud, 20, 20).await;
    for _ in 0..30 {
        timeout(
            Duration::from_secs(1),
            agent.process_inbound_pcm(Bytes::from(silent.clone()), 20),
        )
        .await
        .expect("process_inbound_pcm waited for the blocked finalize")
        .unwrap();
    }
    release.notify_one();
    assert!(agent.wait_stt_idle(Duration::from_secs(10)).await);
    agent.stop().await.unwrap();
}

#[tokio::test]
async fn lid_result_applies_to_closing_utterance() {
    let release = Arc::new(Notify::new());
    let lid_config = LanguageIdConfig {
        enabled: Some(true),
        model_path: Some("/fake/lid-model".into()),
        allowlist: None,
        min_speech_ms: Some(200),
        continuous: None,
        lid_max_clip_ms: None,
        // The close does not wait for the identify beyond this bound.
        lid_gate_max_wait_ms: Some(50),
        timing: None,
        tts_exclusion: Some(true),
    };
    let agent = agent_with(
        Box::new(|| {
            Box::new(InlineStt {
                partial: None,
                pushes: 0,
                partial_sent: false,
            })
        }),
        gate_vad(),
        Some((lid_config, Arc::clone(&release))),
    )
    .await;
    let mut rx = agent.subscribe_events();
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();
    feed(&agent, &loud, 20, 20).await;
    feed(&agent, &silent, 30, 20).await;

    // The utterance is closed (terminal emitted) while its identify is still running.
    let closed = drain(&mut rx);
    let start_id = closed
        .iter()
        .find(|e| e.kind == SpeechEventKind::UserSpeakingStart)
        .and_then(|e| e.utterance_id.clone())
        .expect("utterance started");
    assert!(
        closed
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSttNotFound),
        "the utterance must have ended: {closed:?}"
    );
    assert!(
        !closed
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserLanguage),
        "identify is still gated: {closed:?}"
    );

    release.notify_one();
    let language = timeout(Duration::from_secs(5), async {
        loop {
            let event = rx.recv().await.expect("event bus closed");
            if event.kind == SpeechEventKind::UserLanguage {
                return event;
            }
        }
    })
    .await
    .expect("user_language after the identify was released");
    assert_eq!(language.language.as_deref(), Some("de"));
    assert_eq!(
        language.utterance_id.as_deref(),
        Some(start_id.as_str()),
        "the late language outcome belongs to the utterance it was computed for"
    );
    agent.stop().await.unwrap();
}
