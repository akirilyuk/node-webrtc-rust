//! A late STT final of the previous utterance (the provider answered after finalize gave up and
//! the agent already emitted the fallback final from the last partial) must never be emitted as
//! the next utterance's `user_speech_final`. A mid-phrase pause (brief gap, same utterance) must
//! keep queued results.
//!
//! The scripted `SttProvider` models the cluster client's transcript channel: results queue inside
//! the provider and `poll_transcript` pops them; `discard_queued_transcripts` empties the queue.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
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

#[derive(Default)]
struct Script {
    /// Results queued inside the provider (the transcript channel).
    queue: VecDeque<SttTranscript>,
    /// Queued as a Partial on the next `push_audio`, once.
    partial_on_push: Option<String>,
    /// Queued as a Final on the next `finalize_utterance`, once.
    final_on_finalize: Option<String>,
    discard_calls: usize,
    discarded: usize,
}

struct ScriptedStt {
    script: Arc<Mutex<Script>>,
}

#[async_trait::async_trait]
impl SttProvider for ScriptedStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-late-final"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        let mut script = self.script.lock().unwrap();
        if let Some(text) = script.partial_on_push.take() {
            script.queue.push_back(SttTranscript::Partial(text));
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.script.lock().unwrap().queue.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let mut script = self.script.lock().unwrap();
        if let Some(text) = script.final_on_finalize.take() {
            script.queue.push_back(SttTranscript::Final(text));
        }
        Ok(())
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let mut script = self.script.lock().unwrap();
        let n = script.queue.len();
        script.queue.clear();
        script.discard_calls += 1;
        script.discarded += n;
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

async fn make_agent(script: &Arc<Mutex<Script>>, gate_hold_ms: u32) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(ScriptedFactory {
            script: Arc::clone(script),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = gate_hold_ms;
    vad.barge_in.enabled = false;

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

async fn feed(agent: &Arc<VoiceAgent>, frame: &[u8], n: usize) {
    for _ in 0..n {
        agent
            .process_inbound_pcm(Bytes::from(frame.to_vec()), 20)
            .await
            .unwrap();
    }
}

fn texts_of(
    events: &mut tokio::sync::broadcast::Receiver<node_webrtc_rust_speech::events::SpeechEvent>,
    kind: SpeechEventKind,
) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == kind {
            out.push(event.text.clone().unwrap_or_default());
        }
    }
    out
}

#[tokio::test]
async fn late_final_of_previous_utterance_is_not_emitted_for_next() {
    let script = Arc::new(Mutex::new(Script::default()));
    let agent = make_agent(&script, 80).await;
    let mut events = agent.subscribe_events();
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    // Utterance 1: a partial, and finalize never answers -> fallback final from the partial.
    script.lock().unwrap().partial_on_push = Some("one two".into());
    feed(&agent, &loud, 15).await;
    feed(&agent, &silent, 30).await;

    // The server's real Final for utterance 1 shows up after the fallback was emitted.
    script
        .lock()
        .unwrap()
        .queue
        .push_back(SttTranscript::Final("one two late".into()));

    // Utterance 2.
    {
        let mut s = script.lock().unwrap();
        s.partial_on_push = Some("three four".into());
        s.final_on_finalize = Some("three four".into());
    }
    feed(&agent, &loud, 15).await;
    feed(&agent, &silent, 30).await;

    agent.stop().await.unwrap();

    let finals = texts_of(&mut events, SpeechEventKind::UserSpeechFinal);
    assert_eq!(
        finals,
        vec!["one two".to_string(), "three four".to_string()],
        "late final must not be emitted as a turn"
    );
    let s = script.lock().unwrap();
    assert!(
        s.discarded >= 1,
        "the late final must have been dropped by discard_queued_transcripts (calls={}, dropped={})",
        s.discard_calls,
        s.discarded
    );
}

#[tokio::test]
async fn brief_gap_does_not_drain() {
    let script = Arc::new(Mutex::new(Script::default()));
    // Long hold: a 2-frame pause is a mid-phrase gap of the same utterance.
    let agent = make_agent(&script, 600).await;
    let mut events = agent.subscribe_events();
    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    script.lock().unwrap().partial_on_push = Some("one".into());
    feed(&agent, &loud, 10).await;
    feed(&agent, &silent, 3).await;
    // A result of this same utterance is queued while the speaker pauses.
    script
        .lock()
        .unwrap()
        .queue
        .push_back(SttTranscript::Partial("one two".into()));
    feed(&agent, &loud, 10).await;

    agent.stop().await.unwrap();

    let partials = texts_of(&mut events, SpeechEventKind::UserSpeechPartial);
    assert!(
        partials.iter().any(|t| t == "one two"),
        "queued partial of the same utterance must survive a brief gap, got {partials:?}"
    );
    assert_eq!(
        script.lock().unwrap().discarded,
        0,
        "a brief gap must not drain the provider"
    );
}
