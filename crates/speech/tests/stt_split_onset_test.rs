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

const ALPHA: i16 = 6000;
const ONSET: i16 = 16000;
const BRAVO: i16 = 11000;
const SILENCE: i16 = 0;
/// Room noise: non-zero, below the VAD threshold (0.05 RMS ~ 1638) and below every marker.
const LOOKBACK: i16 = 1000;
/// Mono 16 kHz samples per millisecond.
const SAMPLES_PER_MS: usize = 16;

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
/// Also counts the raw 20 ms "onset" blocks per stream and records the count at finalize in
/// `onset_log` (one entry per finalized stream), so a test can assert which stream got them.
///
/// Records the raw mono samples of each stream in `pcm_log` at finalize (one entry per finalized
/// stream, in order), so a test can assert the exact audio and its order per stream.
#[derive(Default)]
struct MarkerStt {
    words: Vec<&'static str>,
    queue: VecDeque<SttTranscript>,
    onset_blocks: usize,
    onset_log: Arc<Mutex<Vec<usize>>>,
    samples: Vec<i16>,
    pcm_log: Arc<Mutex<Vec<Vec<i16>>>>,
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
        self.samples.extend_from_slice(&samples);
        for block in samples.chunks(320) {
            if let Some(word) = marker_for(block) {
                if word == "onset" {
                    self.onset_blocks += 1;
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
            self.onset_log.lock().unwrap().push(self.onset_blocks);
        }
        if !self.samples.is_empty() {
            self.pcm_log
                .lock()
                .unwrap()
                .push(std::mem::take(&mut self.samples));
        }
        self.words.clear();
        self.onset_blocks = 0;
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
    onset_log: Arc<Mutex<Vec<usize>>>,
    pcm_log: Arc<Mutex<Vec<Vec<i16>>>>,
}

impl VendorFactory for MarkerFactory {
    fn create_stt(
        &self,
        _config: &node_webrtc_rust_speech::SttConfig,
    ) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(MarkerStt {
            onset_log: self.onset_log.clone(),
            pcm_log: self.pcm_log.clone(),
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

async fn make_agent(
    onset_log: Arc<Mutex<Vec<usize>>>,
    pcm_log: Arc<Mutex<Vec<Vec<i16>>>>,
) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(
        SttVendor::Mock,
        Arc::new(MarkerFactory { onset_log, pcm_log }),
    );
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
    let (finals, timeline, _) = run_counting_onset(segments).await;
    (finals, timeline)
}

/// Same as [`run`], plus the number of onset blocks each finalized stream received (in order).
async fn run_counting_onset(
    segments: &[(i16, u64)],
) -> (Vec<String>, Vec<(u64, &'static str, String)>, Vec<usize>) {
    let (finals, timeline, onset_counts, _) = run_full(segments).await;
    (finals, timeline, onset_counts)
}

type Timeline = Vec<(u64, &'static str, String)>;

/// Same as [`run_counting_onset`], plus the raw mono samples each finalized stream received.
async fn run_full(segments: &[(i16, u64)]) -> (Vec<String>, Timeline, Vec<usize>, Vec<Vec<i16>>) {
    let onset_log = Arc::new(Mutex::new(Vec::new()));
    let pcm_log = Arc::new(Mutex::new(Vec::new()));
    let agent = make_agent(onset_log.clone(), pcm_log.clone()).await;
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
    let onset_counts = onset_log.lock().unwrap().clone();
    let streams = pcm_log.lock().unwrap().clone();
    (finals, timeline, onset_counts, streams)
}

/// Run-length encoding of a stream: `(sample value, number of samples)`.
fn runs(samples: &[i16]) -> Vec<(i16, usize)> {
    let mut out: Vec<(i16, usize)> = Vec::new();
    for &s in samples {
        match out.last_mut() {
            Some((v, n)) if *v == s => *n += 1,
            _ => out.push((s, 1)),
        }
    }
    out
}

fn count_level(samples: &[i16], level: i16) -> usize {
    samples.iter().filter(|&&s| s == level).count()
}

fn describe(streams: &[Vec<i16>]) -> String {
    streams
        .iter()
        .enumerate()
        .map(|(i, s)| format!("stream {i}: {:?}", runs(s)))
        .collect::<Vec<_>>()
        .join("\n")
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

/// Hand-off counting (`rt.handoff-double-count`): each onset block is counted by exactly one
/// stream. Long pause: the old stream gets none, the new stream gets all 10 (200 ms / 20 ms).
#[tokio::test]
async fn onset_frames_counted_by_exactly_one_stream() {
    let (finals, timeline, onset_counts) = run_counting_onset(&[
        (ALPHA, 1000),
        (SILENCE, MIN_SILENCE_MS + 700),
        (ONSET, 200),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    let msg = format!(
        "finals: {finals:?}\nonset blocks per stream: {onset_counts:?}\ntimeline: {timeline:?}"
    );
    assert_eq!(onset_counts.len(), 2, "{msg}");
    assert_eq!(onset_counts[0], 0, "old stream must see no onset: {msg}");
    assert_eq!(onset_counts[1], 10, "new stream must see all onset: {msg}");
}

/// Long pause with room noise (LOOKBACK, below the VAD threshold) instead of digital silence. The
/// new stream must start with the real audio before the onset (pre-roll lookback), not with
/// digital zeros: a zero lead-in costs the recognizer the first word ("excuse me" -> "use me").
#[tokio::test]
async fn long_pause_new_stream_starts_with_real_lookback() {
    let (finals, timeline, _, streams) = run_full(&[
        (ALPHA, 1000),
        (LOOKBACK, MIN_SILENCE_MS + 700),
        (ONSET, 200),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    let msg = format!(
        "finals: {finals:?}\n{}\ntimeline: {timeline:?}",
        describe(&streams)
    );
    assert_eq!(
        finals,
        vec!["alpha".to_string(), "onset bravo".to_string()],
        "{msg}"
    );
    assert_eq!(streams.len(), 2, "{msg}");
    let second = runs(&streams[1]);
    assert_eq!(
        second.first().map(|r| r.0),
        Some(LOOKBACK),
        "stream 2 must start with lookback, not padding: {msg}"
    );
    let onset_at = second
        .iter()
        .position(|r| r.0 == ONSET)
        .expect("onset in stream 2");
    assert!(
        second[..onset_at].iter().all(|r| r.0 == LOOKBACK),
        "no digital zeros (or anything else) before the onset in stream 2: {msg}"
    );
}

/// The lookback copy never changes what the old stream gets: stream 1 receives every LOOKBACK
/// sample exactly once, in order, right after alpha (nothing withheld). Stream 2's lead-in is a
/// copy of the last (pre-roll capacity - held onset) of that LOOKBACK, then ONSET. ONSET (speech)
/// reaches only stream 2 (#411).
#[tokio::test]
async fn lookback_copy_leaves_old_stream_unchanged() {
    let lookback_ms = MIN_SILENCE_MS + 700;
    let (finals, timeline, _, streams) = run_full(&[
        (ALPHA, 1000),
        (LOOKBACK, lookback_ms),
        (ONSET, 200),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    let msg = format!(
        "finals: {finals:?}\n{}\ntimeline: {timeline:?}",
        describe(&streams)
    );
    assert_eq!(streams.len(), 2, "{msg}");
    let first = runs(&streams[0]);
    let alpha_at = first.iter().position(|x| x.0 == ALPHA).expect("alpha");
    assert_eq!(
        first[alpha_at..first.len() - 1].to_vec(),
        vec![
            (ALPHA, 1000 * SAMPLES_PER_MS),
            (LOOKBACK, lookback_ms as usize * SAMPLES_PER_MS),
        ],
        "stream 1 gets all lookback once, in order: {msg}"
    );
    assert_eq!(first.last().map(|x| x.0), Some(SILENCE), "{msg}");
    // Pre-roll capacity = speech_pad 200 + min_speech 200. The held onset is min_speech minus the
    // SpeechStart frame itself (which goes to the stream directly).
    let capacity_ms = 200 + 200;
    let held_onset_ms = 200 - FRAME_MS as usize;
    let second = runs(&streams[1]);
    assert_eq!(
        second.iter().take(2).copied().collect::<Vec<_>>(),
        vec![
            (LOOKBACK, (capacity_ms - held_onset_ms) * SAMPLES_PER_MS),
            (ONSET, 200 * SAMPLES_PER_MS),
        ],
        "stream 2 lead-in: {msg}"
    );
    assert_eq!(count_level(&streams[0], ONSET), 0, "{msg}");
}

/// The hold ends with no SpeechStart: the held trailing audio reaches the old stream, in order,
/// before its finalize (before the synthetic endpoint tail), and nothing after it.
#[tokio::test]
async fn hold_without_speech_start_returns_lookback_to_old_stream_in_order() {
    let (finals, timeline, _, streams) = run_full(&[
        (ALPHA, 1000),
        (LOOKBACK, MIN_SILENCE_MS + HOLD_MS + 1200),
        (SILENCE, 2000),
    ])
    .await;
    let msg = format!(
        "finals: {finals:?}\n{}\ntimeline: {timeline:?}",
        describe(&streams)
    );
    assert_eq!(finals, vec!["alpha".to_string()], "{msg}");
    assert_eq!(streams.len(), 1, "{msg}");
    let r = runs(&streams[0]);
    let alpha_at = r.iter().position(|x| x.0 == ALPHA).expect("alpha");
    // Leading pre-roll padding (zeros) is allowed before alpha; after it: alpha, one contiguous
    // lookback run covering at least min_silence + hold, then only the endpoint tail (zeros).
    let after: Vec<i16> = r[alpha_at..].iter().map(|x| x.0).collect();
    assert_eq!(after, vec![ALPHA, LOOKBACK, SILENCE], "{msg}");
    assert_eq!(r[alpha_at].1, 1000 * SAMPLES_PER_MS, "{msg}");
    assert!(
        r[alpha_at + 1].1 >= (MIN_SILENCE_MS + HOLD_MS) as usize * SAMPLES_PER_MS,
        "lookback held during the hold must reach the old stream: {msg}"
    );
}

/// Short pause (within hold/2) with room noise: the same stream continues. Every sample arrives
/// once, in the order it was fed.
#[tokio::test]
async fn brief_gap_lookback_reaches_same_stream_in_order_once() {
    let gap_ms = MIN_SILENCE_MS + 200;
    let (finals, timeline, _, streams) = run_full(&[
        (ALPHA, 1000),
        (LOOKBACK, gap_ms),
        (ONSET, 200),
        (BRAVO, 1000),
        (SILENCE, 3000),
    ])
    .await;
    let msg = format!(
        "finals: {finals:?}\n{}\ntimeline: {timeline:?}",
        describe(&streams)
    );
    assert_eq!(finals, vec!["alpha onset bravo".to_string()], "{msg}");
    assert_eq!(streams.len(), 1, "{msg}");
    let r = runs(&streams[0]);
    let alpha_at = r.iter().position(|x| x.0 == ALPHA).expect("alpha");
    let body: Vec<(i16, usize)> = r[alpha_at..r.len() - 1].to_vec();
    assert_eq!(
        body,
        vec![
            (ALPHA, 1000 * SAMPLES_PER_MS),
            (LOOKBACK, gap_ms as usize * SAMPLES_PER_MS),
            (ONSET, 200 * SAMPLES_PER_MS),
            (BRAVO, 1000 * SAMPLES_PER_MS),
        ],
        "{msg}"
    );
    assert_eq!(r.last().map(|x| x.0), Some(SILENCE), "{msg}");
}
