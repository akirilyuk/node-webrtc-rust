//! Onset of the next phrase must reach exactly one STT stream (shape B).
//!
//! Shape B: after VAD `SpeechEnd` the gate hold keeps feeding the OLD STT stream
//! (`push_to_stt_directly`), including the VAD-pending onset of the NEXT phrase. When that phrase
//! becomes a new utterance (`SpeechStart` with the hold more than half elapsed), the old stream is
//! finalized with the onset inside it, and the new stream is opened from an empty pre-roll ring.
//! The onset word ends up in the old final and is missing from the new one.
//!
//! The scripted STT maps the amplitude of every 20 ms block it receives to a marker word, so the
//! finals show exactly which stream got which audio. The outcome depends only on the audio
//! delivered, never on scheduling.
//!
//! Follow-up: `2026-10-12-stt-split-onset-in-old-stream`.
//!
//! The agent measures some paths with `std::time::Instant`, so frames are paced in REAL time
//! (20 ms per frame) instead of `start_paused = true`.

use std::collections::VecDeque;
use std::sync::Arc;
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

const ALPHA: i16 = 6000;
const ONSET: i16 = 16000;
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
#[derive(Default)]
struct MarkerStt {
    words: Vec<&'static str>,
    queue: VecDeque<SttTranscript>,
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
        }
        self.words.clear();
        Ok(())
    }

    fn discard_queued_transcripts(&mut self) -> usize {
        let n = self.queue.len();
        self.queue.clear();
        n
    }
}

struct MarkerFactory;

impl VendorFactory for MarkerFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(MarkerStt::default()))
    }

    fn create_tts(
        &self,
        config: &TtsConfig,
    ) -> SpeechResult<Box<dyn node_webrtc_rust_speech::TtsProvider>> {
        MockFactory.create_tts(config)
    }
}

async fn make_agent() -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MarkerFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let mut vad = VadConfig {
        enabled: true,
        provider: "energy".into(),
        // alpha (0.18 RMS) clears it, silence does not
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: MIN_SILENCE_MS as u32,
        speech_pad_ms: 200,
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

/// Feed `(level, ms)` segments back to back in real time; return the non-empty user finals in
/// order, plus a `(ms since start, kind, text)` timeline for failure messages.
async fn run(segments: &[(i16, u64)]) -> (Vec<String>, Vec<(u64, &'static str, String)>) {
    let agent = make_agent().await;
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
    (finals, timeline)
}

/// Gap longer than half the hold: the next phrase is a NEW utterance, so its onset belongs to the
/// new stream only.
#[tokio::test]
async fn long_pause_onset_only_in_new_final() {
    let (finals, timeline) = run(&[
        (ALPHA, 1000),
        (SILENCE, MIN_SILENCE_MS + 700),
        (ONSET, 200),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    assert_eq!(
        finals,
        vec!["alpha".to_string(), "onset bravo".to_string()],
        "finals: {finals:?}\ntimeline: {timeline:?}"
    );
}

/// Gap shorter than half the hold: same utterance continues in the same stream.
#[tokio::test]
async fn brief_gap_keeps_one_utterance() {
    let (finals, timeline) = run(&[
        (ALPHA, 1000),
        (SILENCE, MIN_SILENCE_MS + 200),
        (ONSET, 200),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    assert_eq!(
        finals,
        vec!["alpha onset bravo".to_string()],
        "finals: {finals:?}\ntimeline: {timeline:?}"
    );
}

/// Timing: SpeechEnd fires after `min_silence` (300 ms) of silence, so the hold runs from silence
/// ms 300 to 1300. Silence lasts 1200 ms, so a 100 ms onset (below `min_speech` 200 ms: VAD
/// pending only, no SpeechStart) starts at hold ms 900, 100 ms before the hold expires. Then 600
/// ms of silence: the hold expires and the old stream is finalized, then 120 ms of onset (pending
/// total 220 ms >= min_speech, so SpeechStart) and bravo. The first onset slice was fed to the
/// old stream during the hold, the rest goes to the new stream. The pending onset must go to the
/// next stream, not the old final.
#[tokio::test]
async fn hold_expires_during_pending_onset_goes_to_next_stream() {
    let (finals, timeline) = run(&[
        (ALPHA, 1000),
        (SILENCE, MIN_SILENCE_MS + HOLD_MS - 100),
        (ONSET, 100),
        (SILENCE, 600),
        (ONSET, 120),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    let msg = format!("finals: {finals:?}\ntimeline: {timeline:?}");
    assert!(!finals.is_empty(), "{msg}");
    assert_eq!(finals.first().map(String::as_str), Some("alpha"), "{msg}");
    let last = finals.last().unwrap();
    assert!(last.ends_with("bravo"), "{msg}");
    let n = finals.len();
    for f in &finals[..n - 1] {
        assert!(!f.contains("onset"), "{msg}");
    }
}
