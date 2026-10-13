//! `stt_stream_end` carries a close `reason`, and every utterance that opened an STT session ends
//! in exactly one terminal outcome: a non-empty `user_speech_final` or `user_stt_not_found`.
//!
//! The scripted STT emits one partial (possibly blank) once enough LOUD audio arrived and an
//! optional final on `finalize_utterance`. Its output depends only on the audio delivered.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

const FRAME_MS: u64 = 20;
const STREAM_END_REASONS: [&str; 5] = [
    "listen_timeout",
    "finalize_timeout",
    "pcm_stall",
    "end_of_utterance",
    "vendor_final",
];

fn loud_stereo_frame() -> Bytes {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    Bytes::from(pcm)
}

fn silent_stereo_frame() -> Bytes {
    Bytes::from(vec![0_u8; 3840])
}

#[derive(Default)]
struct Script {
    queue: VecDeque<SttTranscript>,
    loud_ms: u32,
    /// Text of the one partial emitted once `partial_after_loud_ms` of loud audio arrived.
    partial: Option<String>,
    partial_after_loud_ms: u32,
    partial_emitted: bool,
    /// Final queued by `finalize_utterance`.
    final_text: Option<String>,
}

struct ScriptedStt {
    script: Arc<Mutex<Script>>,
}

#[async_trait::async_trait]
impl SttProvider for ScriptedStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-close-reason"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let mut script = self.script.lock().unwrap();
        let loud_samples = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .filter(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() > 1000)
            .count() as u32;
        script.loud_ms += loud_samples / 16; // 16 kHz mono
        if !script.partial_emitted && script.loud_ms >= script.partial_after_loud_ms {
            if let Some(text) = script.partial.clone() {
                script.partial_emitted = true;
                script.queue.push_back(SttTranscript::Partial(text));
            }
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.script.lock().unwrap().queue.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let mut script = self.script.lock().unwrap();
        if let Some(text) = script.final_text.clone() {
            script.queue.push_back(SttTranscript::Final(text));
        }
        Ok(())
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let mut script = self.script.lock().unwrap();
        let n = script.queue.len();
        script.queue.clear();
        n
    }
}

struct ScriptedFactory {
    script: Arc<Mutex<Script>>,
}

impl VendorFactory for ScriptedFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(ScriptedStt {
            script: Arc::clone(&self.script),
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

async fn make_agent(
    script: &Arc<Mutex<Script>>,
    gate_stt: bool,
    tweak: impl FnOnce(&mut VadConfig),
) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(ScriptedFactory {
            script: Arc::clone(script),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 100,
        gate_stt,
        stt_gate_hold_ms: 100,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = false;
    tweak(&mut vad);

    let config = VoiceAgentConfig {
        stt: Some(node_webrtc_rust_speech::SttConfig {
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
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

async fn feed(agent: &Arc<VoiceAgent>, frame: &Bytes, frames: usize, paced: bool) {
    for _ in 0..frames {
        agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
        if paced {
            tokio::time::sleep(Duration::from_millis(FRAME_MS)).await;
        }
    }
}

fn drain(events: &mut tokio::sync::broadcast::Receiver<SpeechEvent>) -> Vec<SpeechEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

fn count(events: &[SpeechEvent], kind: SpeechEventKind) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

fn stream_end_reasons(events: &[SpeechEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::SttStreamEnd)
        .map(|e| e.reason.clone())
        .collect()
}

fn finals(events: &[SpeechEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == SpeechEventKind::UserSpeechFinal)
        .map(|e| e.text.clone().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn gate_finalize_with_no_partial_emits_user_stt_not_found() {
    let script = Arc::new(Mutex::new(Script::default()));
    let agent = make_agent(&script, true, |_| {}).await;
    let mut events = agent.subscribe_events();
    feed(&agent, &loud_stereo_frame(), 30, false).await;
    feed(&agent, &silent_stereo_frame(), 60, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = drain(&mut events);
    let _ = agent.stop().await;

    assert_eq!(
        stream_end_reasons(&events),
        vec![Some("end_of_utterance".to_string())],
        "{events:?}"
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "{events:?}"
    );
    assert_eq!(count(&events, SpeechEventKind::UserSttEnd), 1, "{events:?}");
    assert!(finals(&events).is_empty(), "no final expected: {events:?}");
}

#[tokio::test]
async fn forced_close_with_no_partial_text_emits_user_stt_not_found_not_an_empty_final() {
    let script = Arc::new(Mutex::new(Script {
        // A blank partial: the STT session produced no usable transcript text.
        partial: Some("  ".into()),
        partial_after_loud_ms: 200,
        ..Script::default()
    }));
    // Without the gate, C2 is armed right at SpeechEnd and fires on media time.
    let agent = make_agent(&script, false, |vad| {
        vad.utterance_finalize_timeout_ms = 200
    })
    .await;
    let mut events = agent.subscribe_events();
    feed(&agent, &loud_stereo_frame(), 30, false).await;
    feed(&agent, &silent_stereo_frame(), 60, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = drain(&mut events);
    let _ = agent.stop().await;

    assert_eq!(
        stream_end_reasons(&events),
        vec![Some("finalize_timeout".to_string())],
        "{events:?}"
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "{events:?}"
    );
    assert!(
        finals(&events).is_empty(),
        "an empty final must not be emitted: {events:?}"
    );
}

#[tokio::test]
async fn listen_timeout_closes_with_listen_timeout_reason_and_one_not_found() {
    let script = Arc::new(Mutex::new(Script::default()));
    let agent = make_agent(&script, true, |vad| vad.stt_listen_timeout_ms = 300).await;
    let mut events = agent.subscribe_events();
    // Paced so wall time and media time both pass the 300 ms listen timeout.
    feed(&agent, &loud_stereo_frame(), 40, true).await;
    let events = drain(&mut events);
    let _ = agent.stop().await;

    assert_eq!(
        stream_end_reasons(&events),
        vec![Some("listen_timeout".to_string())],
        "{events:?}"
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "{events:?}"
    );
}

#[tokio::test]
async fn vendor_final_closes_with_a_reason_and_no_not_found() {
    let script = Arc::new(Mutex::new(Script {
        partial: Some("hello".into()),
        partial_after_loud_ms: 200,
        final_text: Some("hello world".into()),
        ..Script::default()
    }));
    let agent = make_agent(&script, true, |_| {}).await;
    let mut events = agent.subscribe_events();
    feed(&agent, &loud_stereo_frame(), 30, false).await;
    feed(&agent, &silent_stereo_frame(), 60, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = drain(&mut events);
    let _ = agent.stop().await;

    let reasons = stream_end_reasons(&events);
    assert_eq!(reasons.len(), 1, "{events:?}");
    let reason = reasons[0].as_deref().unwrap_or("");
    assert!(
        STREAM_END_REASONS.contains(&reason),
        "stt_stream_end reason must be one of the five values, got {reasons:?}"
    );
    assert_eq!(
        finals(&events),
        vec!["hello world".to_string()],
        "{events:?}"
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        0,
        "{events:?}"
    );
}

#[tokio::test]
async fn listen_timeout_then_gate_finalize_emits_one_not_found() {
    let script = Arc::new(Mutex::new(Script::default()));
    let agent = make_agent(&script, true, |vad| vad.stt_listen_timeout_ms = 300).await;
    let mut events = agent.subscribe_events();
    feed(&agent, &loud_stereo_frame(), 40, true).await;
    // Speech ends after C1 already closed the stream; the gate finalize must not add a second one.
    feed(&agent, &silent_stereo_frame(), 60, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = drain(&mut events);
    let _ = agent.stop().await;

    assert_eq!(
        stream_end_reasons(&events)
            .first()
            .cloned()
            .flatten()
            .as_deref(),
        Some("listen_timeout"),
        "{events:?}"
    );
    assert_eq!(
        count(&events, SpeechEventKind::UserSttNotFound),
        1,
        "{events:?}"
    );
    assert!(finals(&events).is_empty(), "{events:?}");
}
