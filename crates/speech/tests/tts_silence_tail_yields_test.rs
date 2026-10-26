//! The post-utterance silence tail never delays the next utterance: a reply queued while the
//! tail is being written plays on the next frame, and a reply queued at drain skips the tail.
//!
//! Deterministic: paused tokio time, the writer stamps writes with the virtual clock.

use std::sync::{Arc, Mutex};
use std::time::Duration;

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
use tokio::time::Instant;

const TAIL_MS: u32 = 1500;
const AUDIO_MS: u32 = 200;

struct FixedTts;

fn audio_chunk() -> TtsAudioChunk {
    // 20 ms stereo frames of non-zero PCM (192 bytes per ms).
    TtsAudioChunk {
        pcm: Bytes::from(vec![0x20u8; 192 * AUDIO_MS as usize]),
        duration_ms: AUDIO_MS,
    }
}

#[async_trait]
impl TtsProvider for FixedTts {
    fn vendor_name(&self) -> &'static str {
        "mock-fixed"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        _text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let c = audio_chunk();
        if let Some(sink) = sink {
            let _ = sink.send(c.clone());
        }
        Ok(vec![c])
    }
}

struct FixedFactory;

impl VendorFactory for FixedFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        MockFactory.create_stt(config)
    }

    fn create_tts(&self, _config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(FixedTts))
    }
}

/// (virtual time, frame is silence)
type Writes = Arc<Mutex<Vec<(Instant, bool)>>>;

async fn make_agent() -> (Arc<VoiceAgent>, Writes) {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(FixedFactory));
    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            transport: None,
            sentence_case: Default::default(),
        }),
        post_utterance_silence_ms: Some(TAIL_MS),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&writes);
    let writer: PcmWriter = Arc::new(move |pcm, _ms| {
        sink.lock()
            .unwrap()
            .push((Instant::now(), pcm.iter().all(|b| *b == 0)));
        Ok(())
    });
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    (agent, writes)
}

fn non_blocking() -> SendTextToTtsOptions {
    SendTextToTtsOptions {
        non_blocking: true,
        ..Default::default()
    }
}

fn silent_frames(writes: &Writes) -> usize {
    writes.lock().unwrap().iter().filter(|w| w.1).count()
}

/// Advance virtual time until `n` silent frames were written (the tail is running).
async fn wait_for_silent_frames(writes: &Writes, n: usize) {
    for _ in 0..2000 {
        if silent_frames(writes) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("tail never reached {n} silent frames");
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

#[tokio::test(start_paused = true)]
async fn next_tts_during_silence_tail_starts_without_waiting_for_the_tail() {
    let (agent, writes) = make_agent().await;
    agent
        .send_text_to_tts_with_options("first", non_blocking())
        .await
        .unwrap();
    // 10 silent frames (200 ms) into the 1500 ms tail.
    wait_for_silent_frames(&writes, 10).await;
    let enqueued_at = Instant::now();
    agent
        .send_text_to_tts_with_options("second", non_blocking())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let writes = writes.lock().unwrap().clone();
    let first_audio_after = writes
        .iter()
        .find(|w| w.0 >= enqueued_at && !w.1)
        .expect("second reply never played")
        .0;
    assert!(
        ms(first_audio_after - enqueued_at) <= 40,
        "second reply started {} ms after it was queued (tail left: {} ms)",
        ms(first_audio_after - enqueued_at),
        TAIL_MS - 200
    );
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn queued_tts_at_drain_skips_the_tail() {
    let (agent, writes) = make_agent().await;
    agent
        .send_text_to_tts_with_options("first", non_blocking())
        .await
        .unwrap();
    agent
        .send_text_to_tts_with_options("second", non_blocking())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;

    let writes = writes.lock().unwrap().clone();
    let audio: Vec<usize> = writes
        .iter()
        .enumerate()
        .filter(|(_, w)| !w.1)
        .map(|(i, _)| i)
        .collect();
    // Both replies played (2 x 200 ms = 20 frames).
    assert_eq!(audio.len(), 20, "audio frames: {}", audio.len());
    // No run of silent frames of 100 ms or more between the first and the last audio frame.
    let mut run = 0;
    let mut longest = 0;
    for w in &writes[audio[0]..=*audio.last().unwrap()] {
        if w.1 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    assert!(
        longest * 20 < 100,
        "silence of {} ms between replies",
        longest * 20
    );
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn tail_still_written_after_the_last_utterance() {
    let (agent, writes) = make_agent().await;
    agent
        .send_text_to_tts_with_options("only", non_blocking())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(silent_frames(&writes) * 20, TAIL_MS as usize);
    agent.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn barge_in_during_tail_still_stops_it() {
    let (agent, writes) = make_agent().await;
    agent
        .send_text_to_tts_with_options("only", non_blocking())
        .await
        .unwrap();
    wait_for_silent_frames(&writes, 10).await;
    agent.flush_tts().await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let silent = silent_frames(&writes);
    assert!(
        (10..=12).contains(&silent),
        "tail wrote {silent} silent frames after a flush at 10"
    );
    agent.stop().await.unwrap();
}
