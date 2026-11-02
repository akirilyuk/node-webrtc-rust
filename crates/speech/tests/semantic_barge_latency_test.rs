//! `test.wrong-oracle` (latency not promised by the product): semantic barge-in fires on the
//! **first** partial with `min_stt_partial_tokens` tokens, so its worst-case latency is
//! `SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS` (time to min tokens + one partial interval).
//!
//! The clock here is media time: the fake STT releases partials when the pushed caller audio
//! reaches fixed offsets on a partial grid, so the outcome does not depend on the scheduler.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
    SEMANTIC_BARGE_IN_TIME_TO_MIN_TOKENS_MS, SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS,
    SHERPA_STREAMING_PARTIAL_INTERVAL_MS,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{sleep, timeout, Duration};

/// STT input is 16 kHz mono s16: 32 bytes per ms.
const STT_BYTES_PER_MS: u64 = 32;

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

#[derive(Default)]
struct GridState {
    media_bytes: u64,
    next: usize,
    /// Media ms (since the first pushed STT byte) at which each partial was released.
    released_at_ms: Vec<u64>,
}

/// Releases `partials[i]` once pushed media reaches `at_ms[i]`.
struct GridStt {
    partials: Vec<(&'static str, u64)>,
    state: Arc<Mutex<GridState>>,
}

#[async_trait::async_trait]
impl SttProvider for GridStt {
    fn vendor_name(&self) -> &'static str {
        "partial-grid"
    }

    async fn start(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> node_webrtc_rust_speech::SpeechResult<()> {
        self.state.lock().unwrap().media_bytes += pcm.len() as u64;
        Ok(())
    }

    async fn poll_transcript(
        &mut self,
    ) -> node_webrtc_rust_speech::SpeechResult<Option<SttTranscript>> {
        let mut st = self.state.lock().unwrap();
        let media_ms = st.media_bytes / STT_BYTES_PER_MS;
        let Some((text, at_ms)) = self.partials.get(st.next).copied() else {
            return Ok(None);
        };
        if media_ms < at_ms {
            return Ok(None);
        }
        st.next += 1;
        st.released_at_ms.push(media_ms);
        Ok(Some(SttTranscript::Partial(text.into())))
    }

    async fn stop(&mut self) -> node_webrtc_rust_speech::SpeechResult<()> {
        Ok(())
    }
}

struct GridFactory {
    partials: Vec<(&'static str, u64)>,
    state: Arc<Mutex<GridState>>,
}

impl VendorFactory for GridFactory {
    fn create_stt(
        &self,
        _config: &SttConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(GridStt {
            partials: self.partials.clone(),
            state: Arc::clone(&self.state),
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> node_webrtc_rust_speech::SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

fn config() -> VoiceAgentConfig {
    let mut vad = VadConfig {
        enabled: true,
        threshold: 0.05,
        min_speech_duration_ms: 40,
        min_silence_duration_ms: 40,
        gate_stt: true,
        ..VadConfig::default()
    };
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.flush_tts = true;
    vad.barge_in.agent_playback_guard_ms = 0;
    vad.barge_in.require_stt_partial = true;
    assert_eq!(vad.barge_in.min_stt_partial_tokens, 2);
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
    }
}

struct Outcome {
    kinds: Vec<SpeechEventKind>,
    released_at_ms: Vec<u64>,
}

/// Plays a long reply, feeds loud caller frames until `barge_in` (bounded), returns events.
async fn run(partials: Vec<(&'static str, u64)>) -> Outcome {
    let state = Arc::new(Mutex::new(GridState::default()));
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(GridFactory {
            partials,
            state: Arc::clone(&state),
        }),
    );
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let agent = Arc::new(VoiceAgent::new(config(), Arc::new(registry)).unwrap());
    let mut events = agent.subscribe_events();
    let seen: Arc<Mutex<Vec<SpeechEventKind>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_collector = Arc::clone(&seen);
    let collector = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            seen_collector.lock().unwrap().push(event.kind);
        }
    });

    // Real-time paced playback so the reply outlasts the caller frames.
    let writer: PcmWriter = Arc::new(move |_pcm, _ms| {
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(20)));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();

    let text = "keep talking while the caller interrupts this long mock reply ".repeat(20);
    let speak_agent = Arc::clone(&agent);
    let speak = tokio::spawn(async move { speak_agent.send_text_to_tts(&text).await });

    let has = |kind: SpeechEventKind| seen.lock().unwrap().contains(&kind);
    timeout(Duration::from_secs(5), async {
        while !has(SpeechEventKind::AgentSpeakingStart) {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("agent_speaking_start within 5 s");

    let loud = loud_stereo_frame();
    // Bound: 400 frames = 8 s of caller audio, far beyond the worst-case latency.
    for _ in 0..400 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(1)).await;
        if has(SpeechEventKind::BargeIn) {
            break;
        }
    }
    sleep(Duration::from_millis(200)).await;
    let _ = timeout(Duration::from_secs(5), agent.stop()).await;
    speak.abort();
    collector.abort();
    let kinds = seen.lock().unwrap().clone();
    let released_at_ms = state.lock().unwrap().released_at_ms.clone();
    Outcome {
        kinds,
        released_at_ms,
    }
}

fn partials_before_barge(kinds: &[SpeechEventKind]) -> usize {
    let barge = kinds
        .iter()
        .position(|k| *k == SpeechEventKind::BargeIn)
        .unwrap_or_else(|| panic!("no barge_in in {kinds:?}"));
    kinds[..barge]
        .iter()
        .filter(|k| **k == SpeechEventKind::UserSpeechPartial)
        .count()
}

#[test]
fn worst_case_latency_is_time_to_min_tokens_plus_one_partial_interval() {
    assert_eq!(SHERPA_STREAMING_PARTIAL_INTERVAL_MS, 1_280);
    assert_eq!(SEMANTIC_BARGE_IN_TIME_TO_MIN_TOKENS_MS, 1_440);
    assert_eq!(SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS, 2_720);
}

#[test]
fn sdk_constant_matches_rust_constant() {
    let sdk = include_str!("../../../packages/sdk/src/voice/defaults.ts");
    let needle = format!(
        "export const SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS = {}",
        SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS
    );
    assert!(
        sdk.contains(&needle),
        "SDK defaults.ts must contain `{needle}`"
    );
}

/// Worst grid phase: the first partial after onset holds one token ("excuse"), so barge waits for
/// the next partial, one interval later, and lands exactly on the documented bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_token_first_partial_barges_on_next_partial_within_bound() {
    let first = SEMANTIC_BARGE_IN_TIME_TO_MIN_TOKENS_MS as u64;
    let second = first + SHERPA_STREAMING_PARTIAL_INTERVAL_MS as u64;
    let out = run(vec![("excuse", first), ("excuse me", second)]).await;
    assert_eq!(
        partials_before_barge(&out.kinds),
        2,
        "one-token partial must not barge; second (2-token) partial must: {:?}",
        out.kinds
    );
    let qualifying_ms = out.released_at_ms[1];
    assert!(
        qualifying_ms <= SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS as u64,
        "qualifying partial at {qualifying_ms} ms > bound {SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS} ms"
    );
}

/// Good grid phase: the first partial already has two tokens and barge fires on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_token_first_partial_barges_immediately() {
    let first = SEMANTIC_BARGE_IN_TIME_TO_MIN_TOKENS_MS as u64;
    let out = run(vec![("excuse me", first)]).await;
    assert_eq!(partials_before_barge(&out.kinds), 1, "{:?}", out.kinds);
    assert!(out.released_at_ms[0] <= SEMANTIC_BARGE_IN_WORST_CASE_LATENCY_MS as u64);
}
