//! Time to first audio on `agent_speaking_start` (`first_chunk_ms`, `first_audio_ms`).
//!
//! Only lower bounds are asserted on wall-clock durations so loaded runners cannot flake.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttConfig, TtsConfig, TtsVendor, VoiceAgentConfig,
};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_speech::{PcmWriter, SttProvider, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

const CHUNK_MS: u32 = 200;

/// TTS that waits `first_chunk_delay` before emitting one PCM chunk (the vendor TTFB).
struct DelayedTts {
    first_chunk_delay: Duration,
    synth_started: Arc<AtomicBool>,
}

fn sine_chunk(duration_ms: u32) -> TtsAudioChunk {
    let samples = (48_000 * duration_ms / 1000) as usize;
    let mut pcm = Vec::with_capacity(samples * 4);
    for i in 0..samples {
        let t = i as f32 / 48_000.0;
        let sample =
            ((t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.2 * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    TtsAudioChunk {
        pcm: Bytes::from(pcm),
        duration_ms,
    }
}

#[async_trait]
impl TtsProvider for DelayedTts {
    fn vendor_name(&self) -> &'static str {
        "mock-delayed"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        _text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synth_started.store(true, Ordering::SeqCst);
        tokio::time::sleep(self.first_chunk_delay).await;
        let chunk = sine_chunk(CHUNK_MS);
        if let Some(sink) = sink {
            let _ = sink.send(chunk.clone());
        }
        Ok(vec![chunk])
    }
}

struct DelayedFactory {
    first_chunk_delay: Duration,
    synth_started: Arc<AtomicBool>,
}

impl VendorFactory for DelayedFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(DelayedTts {
            first_chunk_delay: self.first_chunk_delay,
            synth_started: Arc::clone(&self.synth_started),
        }))
    }
}

async fn make_agent(first_chunk_delay: Duration) -> (Arc<VoiceAgent>, Arc<AtomicBool>) {
    let synth_started = Arc::new(AtomicBool::new(false));
    let mut registry = VendorRegistry::new();
    registry.register_tts(
        TtsVendor::Mock,
        Arc::new(DelayedFactory {
            first_chunk_delay,
            synth_started: Arc::clone(&synth_started),
        }),
    );
    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, synth_started)
}

/// Next `agent_speaking_start` on `events` (event-driven, bounded by a generous timeout).
async fn next_speaking_start(events: &mut Receiver<SpeechEvent>) -> SpeechEvent {
    timeout(Duration::from_secs(30), async {
        loop {
            let event = events.recv().await.expect("event bus closed");
            if event.kind == SpeechEventKind::AgentSpeakingStart {
                return event;
            }
        }
    })
    .await
    .expect("timed out waiting for agent_speaking_start")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_speaking_start_reports_first_chunk_and_first_audio() {
    let (agent, _) = make_agent(Duration::from_millis(80)).await;
    let mut events = agent.subscribe_events();

    agent.send_text_to_tts("hello there").await.unwrap();
    let start = next_speaking_start(&mut events).await;

    let first_chunk_ms = start.first_chunk_ms.expect("first_chunk_ms set");
    let first_audio_ms = start.first_audio_ms.expect("first_audio_ms set");
    assert!(first_chunk_ms >= 80, "first_chunk_ms={first_chunk_ms}");
    assert!(
        first_audio_ms >= first_chunk_ms,
        "first_audio_ms={first_audio_ms} first_chunk_ms={first_chunk_ms}"
    );
    assert!(
        !agent.has_pending_first_audio().await,
        "timing is consumed by agent_speaking_start"
    );
    agent.stop().await.unwrap();
}

/// The synthesis worker runs one job at a time and waits for playback, so a reply sent while
/// another is playing starts its own playback afterwards. Its timing is measured from its own
/// request (it includes the wait), and nothing is left pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_reply_while_speaking_is_timed_from_its_own_request() {
    let (agent, _) = make_agent(Duration::from_millis(80)).await;
    let mut events = agent.subscribe_events();
    let non_blocking = SendTextToTtsOptions {
        non_blocking: true,
        ..Default::default()
    };

    agent
        .send_text_to_tts_with_options("first reply", non_blocking)
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("second reply", non_blocking)
        .await
        .unwrap();

    let first = next_speaking_start(&mut events).await;
    let second = next_speaking_start(&mut events).await;
    agent.wait_tts_playback_idle().await.unwrap();

    let first_audio = first.first_audio_ms.expect("first reply first_audio_ms");
    let second_chunk = second.first_chunk_ms.expect("second reply first_chunk_ms");
    let second_audio = second.first_audio_ms.expect("second reply first_audio_ms");
    assert!(second_chunk >= 80, "second first_chunk_ms={second_chunk}");
    assert!(
        second_chunk >= first_audio,
        "second reply waited behind the first: {second_chunk} < {first_audio}"
    );
    assert!(second_audio >= second_chunk);
    assert!(!agent.has_pending_first_audio().await);
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn barge_in_clears_pending_first_audio() {
    let (agent, synth_started) = make_agent(Duration::from_millis(300)).await;
    let mut events = agent.subscribe_events();
    let non_blocking = SendTextToTtsOptions {
        non_blocking: true,
        ..Default::default()
    };

    let first_requested_at = Instant::now();
    agent
        .send_text_to_tts_with_options("first reply", non_blocking)
        .await
        .unwrap();
    // The vendor call is running, so the first reply has pending timing but no audio yet.
    timeout(Duration::from_secs(30), async {
        while !synth_started.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("synthesis never started");
    assert!(agent.has_pending_first_audio().await);

    agent.flush_tts().await.unwrap();
    assert!(
        !agent.has_pending_first_audio().await,
        "flush must clear pending timing"
    );

    // Make the second request provably later than the first.
    tokio::time::sleep(Duration::from_millis(30)).await;
    agent
        .send_text_to_tts_with_options("second reply", non_blocking)
        .await
        .unwrap();

    let start = next_speaking_start(&mut events).await;
    let since_first_request = first_requested_at.elapsed().as_millis() as u32;
    let first_audio_ms = start.first_audio_ms.expect("first_audio_ms set");
    assert!(
        first_audio_ms + 20 <= since_first_request,
        "first_audio_ms={first_audio_ms} must be measured from the second request \
         (elapsed since first request: {since_first_request})"
    );
    agent.stop().await.unwrap();
}
