//! A short, quiet first word (below `min_speech_duration_ms`, so no VAD `SpeechStart` of its own)
//! must reach the STT stream of the next utterance, even when the pause between them is longer
//! than the pre-roll ring, and must be counted by exactly one stream.
//!
//! The scripted STT maps the amplitude of every 20 ms block to a marker word and counts the raw
//! `alpha` blocks per finalized stream. Frames are paced in REAL time (20 ms per frame).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

const FRAME_MS: u64 = 20;
const MIN_SILENCE_MS: u64 = 300;
const HOLD_MS: u64 = 1000;
const FRAME: u64 = 20;

const ALPHA: i16 = 6000;
const BRAVO: i16 = 11000;
const SILENCE: i16 = 0;

/// Constant-amplitude 48 kHz stereo 20 ms frame (`stereo_48k_to_mono_16k` averages channels and
/// then triples, so a constant stays the same constant).
fn stereo_frame(level: i16) -> Bytes {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&level.to_le_bytes());
        pcm.extend_from_slice(&level.to_le_bytes());
    }
    Bytes::from(pcm)
}

fn marker_for(block: &[i16]) -> Option<&'static str> {
    let max = block.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    if max >= 14000 {
        Some("onset")
    } else if max >= 9000 {
        Some("bravo")
    } else if max >= 4000 {
        Some("alpha")
    } else {
        None
    }
}

/// One transcript per stream: words are the collapsed markers of the audio pushed since the last
/// `finalize_utterance`.
///
/// Also counts the raw 20 ms "alpha" blocks per stream and records the count at finalize in
/// `alpha_log` (one entry per finalized stream), so a test can assert which stream got them.
#[derive(Default)]
struct MarkerStt {
    words: Vec<&'static str>,
    queue: VecDeque<SttTranscript>,
    alpha_blocks: usize,
    alpha_log: Arc<Mutex<Vec<usize>>>,
}

#[async_trait::async_trait]
impl SttProvider for MarkerStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-markers"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let samples: Vec<i16> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        for block in samples.chunks(320) {
            if let Some(word) = marker_for(block) {
                if word == "alpha" {
                    self.alpha_blocks += 1;
                }
                if self.words.last() != Some(&word) {
                    self.words.push(word);
                    self.queue
                        .push_back(SttTranscript::Partial(self.words.join(" ")));
                }
            }
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(self.queue.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        if !self.words.is_empty() {
            self.queue
                .push_back(SttTranscript::Final(self.words.join(" ")));
            self.alpha_log.lock().unwrap().push(self.alpha_blocks);
        }
        self.words.clear();
        self.alpha_blocks = 0;
        Ok(())
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let n = self.queue.len();
        self.queue.clear();
        n
    }
}

#[derive(Default)]
struct MarkerFactory {
    alpha_log: Arc<Mutex<Vec<usize>>>,
}

impl VendorFactory for MarkerFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(MarkerStt {
            alpha_log: self.alpha_log.clone(),
            ..Default::default()
        }))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

async fn make_agent(alpha_log: Arc<Mutex<Vec<usize>>>) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MarkerFactory { alpha_log }));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig {
        enabled: true,
        provider: "energy".into(),
        // alpha (0.18 RMS) clears it, silence does not; ring = 500 + 200 = 700 ms
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: MIN_SILENCE_MS as u32,
        speech_pad_ms: 500,
        gate_stt: true,
        stt_gate_hold_ms: HOLD_MS as u32,
        ..Default::default()
    };
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

/// Feed `(level, ms)` segments back to back in real time; return the non-empty user finals, a
/// timeline for failure messages, and the number of onset blocks each finalized stream received (in order).
async fn run_counting_alpha(
    segments: &[(i16, u64)],
) -> (Vec<String>, Vec<(u64, &'static str, String)>, Vec<usize>) {
    let alpha_log = Arc::new(Mutex::new(Vec::new()));
    let agent = make_agent(alpha_log.clone()).await;
    let mut events = agent.subscribe_events();
    let start = std::time::Instant::now();
    let mut timeline = Vec::new();
    let mut i: u64 = 0;

    let drain = |events: &mut tokio::sync::broadcast::Receiver<_>,
                 timeline: &mut Vec<(u64, &'static str, String)>| {
        while let Ok(event) = events.try_recv() {
            let event: node_webrtc_rust_speech::events::SpeechEvent = event;
            let at = start.elapsed().as_millis() as u64;
            match event.kind {
                SpeechEventKind::UserSpeechPartial => {
                    timeline.push((at, "partial", event.text.clone().unwrap_or_default()));
                }
                SpeechEventKind::UserSpeechFinal => {
                    timeline.push((at, "final", event.text.clone().unwrap_or_default()));
                }
                _ => {}
            }
        }
    };

    for (level, ms) in segments {
        let frame = stereo_frame(*level);
        for _ in 0..(ms / FRAME_MS) {
            let due = start + Duration::from_millis(i * FRAME_MS);
            tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
            agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
            drain(&mut events, &mut timeline);
            i += 1;
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    drain(&mut events, &mut timeline);
    let _ = agent.stop().await;

    let finals = timeline
        .iter()
        .filter(|(_, k, t)| *k == "final" && !t.is_empty())
        .map(|(_, _, t)| t.clone())
        .collect();
    let alpha_counts = alpha_log.lock().unwrap().clone();
    (finals, timeline, alpha_counts)
}

fn silence(frames: u64) -> (i16, u64) {
    (SILENCE, frames * FRAME)
}

fn burst(level: i16, frames: u64) -> (i16, u64) {
    (level, frames * FRAME)
}

async fn check(segments: &[(i16, u64)], expected_finals: &[&str], expected_alpha_blocks: &[usize]) {
    let (finals, timeline, alpha) = run_counting_alpha(segments).await;
    let msg =
        format!("finals: {finals:?}\nalpha blocks per stream: {alpha:?}\ntimeline: {timeline:?}");
    assert_eq!(
        finals,
        expected_finals
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "{msg}"
    );
    assert_eq!(alpha, expected_alpha_blocks, "{msg}");
}

#[tokio::test]
async fn short_quiet_first_word_after_long_pause_is_kept() {
    check(
        &[
            silence(25),
            burst(ALPHA, 8),
            silence(70),
            burst(BRAVO, 40),
            silence(150),
        ],
        &["alpha bravo"],
        &[8],
    )
    .await;
}

#[tokio::test]
async fn short_quiet_first_word_after_2500ms_pause_is_kept() {
    check(
        &[
            silence(25),
            burst(ALPHA, 8),
            silence(125),
            burst(BRAVO, 40),
            silence(150),
        ],
        &["alpha bravo"],
        &[8],
    )
    .await;
}

#[tokio::test]
async fn short_burst_still_in_ring_is_not_duplicated() {
    check(
        &[
            silence(25),
            burst(ALPHA, 8),
            silence(15),
            burst(BRAVO, 40),
            silence(150),
        ],
        &["alpha bravo"],
        &[8],
    )
    .await;
}

#[tokio::test]
async fn short_burst_partly_out_of_ring_is_not_duplicated() {
    check(
        &[
            silence(25),
            burst(ALPHA, 8),
            silence(30),
            burst(BRAVO, 40),
            silence(150),
        ],
        &["alpha bravo"],
        &[8],
    )
    .await;
}

#[tokio::test]
async fn short_burst_older_than_window_is_not_kept() {
    check(
        &[
            silence(25),
            burst(ALPHA, 8),
            silence(175),
            burst(BRAVO, 40),
            silence(150),
        ],
        &["bravo"],
        &[0],
    )
    .await;
}

#[tokio::test]
async fn lone_short_blip_opens_no_utterance() {
    let onset_log = Arc::new(Mutex::new(Vec::new()));
    let agent = make_agent(onset_log).await;
    let mut events = agent.subscribe_events();
    let start = std::time::Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let frames: Vec<i16> = [(SILENCE, 25), (ALPHA, 3), (SILENCE, 150)]
        .iter()
        .flat_map(|(l, n)| std::iter::repeat_n(*l, *n))
        .collect();
    for (i, level) in (0_u64..).zip(frames) {
        let due = start + Duration::from_millis(i * FRAME_MS);
        tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
        agent
            .process_inbound_pcm(stereo_frame(level), 20)
            .await
            .unwrap();
        while let Ok(event) = events.try_recv() {
            let event: node_webrtc_rust_speech::events::SpeechEvent = event;
            match event.kind {
                SpeechEventKind::UserSpeakingStart
                | SpeechEventKind::UserSpeechFinal
                | SpeechEventKind::UserSttNotFound => seen.push(format!("{:?}", event.kind)),
                _ => {}
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(event) = events.try_recv() {
        let event: node_webrtc_rust_speech::events::SpeechEvent = event;
        if matches!(
            event.kind,
            SpeechEventKind::UserSpeakingStart
                | SpeechEventKind::UserSpeechFinal
                | SpeechEventKind::UserSttNotFound
        ) {
            seen.push(format!("{:?}", event.kind));
        }
    }
    let _ = agent.stop().await;
    assert!(seen.is_empty(), "unexpected events: {seen:?}");
}
