//! A vendor closes an STT stream that got no audio for a while (measured 2026-10-19: ElevenLabs
//! after about 15 s, Deepgram before 20 s). The gate sends nothing to STT between utterances, so a session that sits
//! idle before the caller speaks has a dead stream when the first word arrives.
//!
//! Staging 2026-10-19 (nwr 0.11.6): ElevenLabs turns ended with `user_stt_not_found` /
//! `stt_stream_end{listen_timeout}`, zero partials, after 4 s: the audio went to a closed socket
//! and the C1 window ran against a stream that could not answer. The STT actor reopens a stream
//! whose provider reports `stream_alive() == false` before the next audio, with the open marked
//! pending, so C1 waits for it.
//!
//! Timings are scaled from the runner's (C1 window 4000 ms, vendor first partial 2500 ms after the
//! first audio) by 0.2: window 800 ms, first partial 500 ms.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SpeechError, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::Instant;

const COUNTING: &str = "one two three four five six seven eight nine ten";
const WINDOW_MS: u32 = 800;
const FIRST_PARTIAL_AFTER_AUDIO_MS: u64 = 500;
const OPEN_MS: u64 = 300;

#[derive(Default)]
struct Probe {
    /// The stream is open and accepts audio.
    alive: AtomicBool,
    starts: AtomicUsize,
    /// Audio pushed while the stream was closed (lost).
    lost_pushes: AtomicUsize,
    /// `start` fails from the second call on.
    reopen_fails: AtomicBool,
    first_push_at: Mutex<Option<Instant>>,
}

impl Probe {
    /// The vendor's idle close.
    fn vendor_closes_stream(&self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

struct Idle {
    probe: Arc<Probe>,
    partial_sent: bool,
    finalized: bool,
    final_sent: bool,
}

#[async_trait::async_trait]
impl SttProvider for Idle {
    fn vendor_name(&self) -> &'static str {
        "scripted-idle-close"
    }

    fn stream_alive(&self) -> bool {
        self.probe.alive.load(Ordering::SeqCst)
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let n = self.probe.starts.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::time::sleep(Duration::from_millis(OPEN_MS)).await;
        if n > 1 && self.probe.reopen_fails.load(Ordering::SeqCst) {
            return Err(SpeechError::Stt("scripted reopen failure".into()));
        }
        self.probe.alive.store(true, Ordering::SeqCst);
        self.partial_sent = false;
        self.finalized = false;
        self.final_sent = false;
        *self.probe.first_push_at.lock().unwrap() = None;
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        if !self.probe.alive.load(Ordering::SeqCst) {
            self.probe.lost_pushes.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        self.probe
            .first_push_at
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let first = *self.probe.first_push_at.lock().unwrap();
        if !self.partial_sent {
            if let Some(at) = first {
                if at.elapsed() >= Duration::from_millis(FIRST_PARTIAL_AFTER_AUDIO_MS) {
                    self.partial_sent = true;
                    return Ok(Some(SttTranscript::Partial("one two".into())));
                }
            }
        }
        if self.finalized && !self.final_sent && first.is_some() {
            self.final_sent = true;
            return Ok(Some(SttTranscript::Final(COUNTING.into())));
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        self.finalized = true;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.probe.alive.store(false, Ordering::SeqCst);
        Ok(())
    }
}

struct IdleFactory(Arc<Probe>);

impl VendorFactory for IdleFactory {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(Idle {
            probe: Arc::clone(&self.0),
            partial_sent: false,
            finalized: false,
            final_sent: false,
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn loud_frame(duration_ms: usize) -> Vec<u8> {
    let samples = 48 * duration_ms;
    let mut pcm = Vec::with_capacity(samples * 4);
    for _ in 0..samples {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn config() -> VoiceAgentConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        stt_gate_hold_ms: 80,
        stt_listen_timeout_ms: WINDOW_MS,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = false;
    VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
            transport: None,
        }),
        tts: None,
        vad,
        ..Default::default()
    }
}

/// Starts the agent, lets the vendor close the stream (when `close`), then speaks for 1.2 s in
/// 100 ms frames paced at real time, followed by silence. Returns the events.
async fn run(probe: Arc<Probe>, close: bool) -> Vec<SpeechEvent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(IdleFactory(Arc::clone(&probe))));
    let agent = VoiceAgent::new(config(), Arc::new(registry)).unwrap();
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    if close {
        probe.vendor_closes_stream();
    }

    let mut seen = Vec::new();
    let loud = loud_frame(100);
    for _ in 0..12 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 100)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
    }
    let silent = vec![0_u8; 3840];
    for _ in 0..60 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
    }
    assert!(agent.wait_stt_idle(Duration::from_secs(10)).await);
    agent.stop().await.unwrap();
    while let Ok(event) = events.try_recv() {
        seen.push(event);
    }
    seen
}

fn count(events: &[SpeechEvent], kind: SpeechEventKind) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

fn kinds(events: &[SpeechEvent]) -> Vec<SpeechEventKind> {
    events.iter().map(|e| e.kind).collect()
}

/// The vendor answers its first partial 500 ms (scaled 2.5 s) after the first audio, inside the
/// 800 ms (scaled 4 s) window, on a stream that is open: one final, no `user_stt_not_found`.
#[tokio::test]
async fn slow_first_partial_on_an_open_stream_ends_with_one_final() {
    let probe = Arc::new(Probe::default());
    let events = run(Arc::clone(&probe), false).await;
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        0,
        "{:?}",
        kinds(&events)
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSpeechFinal),
        1,
        "{:?}",
        kinds(&events)
    );
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
}

/// The staging failure: the stream was closed by the vendor while the session was idle. The audio
/// must reach a reopened stream (none lost), the 300 ms open must not eat the C1 window, and the
/// turn ends with one final.
#[tokio::test]
async fn stream_closed_by_the_vendor_while_idle_is_reopened_before_the_audio() {
    let probe = Arc::new(Probe::default());
    let events = run(Arc::clone(&probe), true).await;
    assert_eq!(
        probe.lost_pushes.load(Ordering::SeqCst),
        0,
        "audio went to a closed stream"
    );
    assert_eq!(probe.starts.load(Ordering::SeqCst), 2, "one reopen");
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        0,
        "{:?}",
        kinds(&events)
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSpeechFinal),
        1,
        "{:?}",
        kinds(&events)
    );
}

/// A reopen that fails is attempted once per backoff period, not once per audio frame, and the
/// turn still ends (C1 `user_stt_not_found`) instead of hanging.
#[tokio::test]
async fn failed_reopen_is_not_retried_per_frame() {
    let probe = Arc::new(Probe::default());
    probe.reopen_fails.store(true, Ordering::SeqCst);
    let events = run(Arc::clone(&probe), true).await;
    let starts = probe.starts.load(Ordering::SeqCst);
    assert!((2..=3).contains(&starts), "start attempts: {starts}");
    assert_eq!(
        count(&events, SpeechEventKind::UserSpeechFinal),
        0,
        "{:?}",
        kinds(&events)
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "{:?}",
        kinds(&events)
    );
}
