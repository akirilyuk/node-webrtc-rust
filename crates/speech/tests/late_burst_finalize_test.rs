//! Late-burst audio must produce the same finals as real-time audio (diagnosis, chunk 4a).
//!
//! Follow-up: STT splits or loses words when audio is processed late (CPU-stalled host). The
//! end-of-turn finalize (C2) runs on media time in `process_inbound_pcm` and on a 50 ms wall-clock
//! ticker (`c2_wall_clock_tick`). These tests feed ONE utterance three ways and compare the finals:
//!
//! - `realtime`: one 20 ms frame per 20 ms of wall time (the baseline);
//! - `stall in speech`: first 1 s real time, then a 2 s wall stall with no PCM delivered, then the
//!   queued backlog as a burst, then real time again;
//! - `stall in end-of-turn silence`: all speech delivered, then a 2 s wall stall, then the queued
//!   silence as a burst.
//!
//! The agent measures wall time with `std::time::Instant`, which `tokio::time::pause` does not
//! control, so these tests run in REAL time (about 8 s each, parallel) instead of
//! `start_paused = true`.
//!
//! The scripted STT emits one partial per 500 ms of LOUD audio it receives and, on
//! `finalize_utterance`, a final holding every word so far. Its output depends only on the audio
//! delivered, never on timing, so any difference between runs is the agent's segmentation.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_speech::{PcmWriter, SpeechResult, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

const FRAME_MS: u64 = 20;
const SPEECH_FRAMES: usize = 150; // 3 s of speech
/// min_silence 1300 + gate hold 1000 + slack for the finalize and the second-half of C2.
const SILENCE_FRAMES: usize = 250; // 5 s

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
    queue: VecDeque<SttTranscript>,
    loud_ms: u32,
    words_emitted: u32,
    /// Polls left before the queued final becomes visible (models decode lag after finalize).
    final_in_polls: Option<(u32, String)>,
    final_delay_polls: u32,
    /// Sleep inside `push_audio` for chunks of at least 3200 bytes (the 100 ms endpoint-tail
    /// chunks), modelling a decode that is starved of CPU while the inbound call is running.
    tail_push_delay_ms: u64,
    finalize_calls: u32,
}

fn words(n: u32) -> String {
    (1..=n)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct ScriptedStt {
    script: Arc<Mutex<Script>>,
}

#[async_trait::async_trait]
impl SttProvider for ScriptedStt {
    fn vendor_name(&self) -> &'static str {
        "scripted-late-burst"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let delay = {
            let mut script = self.script.lock().unwrap();
            let loud_samples = pcm
                .chunks_exact(2)
                .filter(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() > 1000)
                .count() as u32;
            script.loud_ms += loud_samples / 16; // 16 kHz mono
            let target = script.loud_ms / 500;
            while script.words_emitted < target {
                script.words_emitted += 1;
                let text = words(script.words_emitted);
                script.queue.push_back(SttTranscript::Partial(text));
            }
            if pcm.len() >= 3200 {
                script.tail_push_delay_ms
            } else {
                0
            }
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut script = self.script.lock().unwrap();
        if let Some((left, text)) = script.final_in_polls.take() {
            if left == 0 {
                script.queue.push_back(SttTranscript::Final(text));
            } else {
                script.final_in_polls = Some((left - 1, text));
            }
        }
        Ok(script.queue.pop_front())
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let mut script = self.script.lock().unwrap();
        script.finalize_calls += 1;
        let text = words(script.loud_ms / 500);
        script.loud_ms = 0;
        script.words_emitted = 0;
        let delay = script.final_delay_polls;
        script.final_in_polls = Some((delay, text));
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

/// Production-shaped VAD (see `roundtrip-counting-echo-lid-multi`): 1300 ms min silence,
/// 1000 ms STT gate hold, default 1500 ms C2 timeout.
async fn make_agent(script: &Arc<Mutex<Script>>) -> Arc<VoiceAgent> {
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
    vad.min_silence_duration_ms = 1300;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 1000;
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

/// Stall: after `after_frames` frames have been delivered, deliver nothing for `stall_ms` of
/// wall time. Frames keep their original schedule (`start + i * 20 ms`), so the backlog is
/// delivered as a burst once the stall ends and real-time pacing resumes when it catches up.
#[derive(Clone, Copy)]
struct Stall {
    after_frames: usize,
    stall_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    finals: Vec<String>,
    partial_count: usize,
    finalize_calls: u32,
    /// `(ms since first frame, kind, text)` of every partial / final, for failure messages.
    timeline: Vec<(u64, &'static str, String)>,
}

async fn run(stall: Option<Stall>, setup: impl FnOnce(&mut Script)) -> Observed {
    run_segments(
        &[(true, SPEECH_FRAMES), (false, SILENCE_FRAMES)],
        stall,
        setup,
    )
    .await
}

/// `segments`: `(loud, frame_count)` runs delivered back to back on the 20 ms schedule.
async fn run_segments(
    segments: &[(bool, usize)],
    stall: Option<Stall>,
    setup: impl FnOnce(&mut Script),
) -> Observed {
    let script = Arc::new(Mutex::new(Script::default()));
    setup(&mut script.lock().unwrap());
    let agent = make_agent(&script).await;
    let mut events = agent.subscribe_events();
    let loud = Bytes::from(loud_stereo_frame());
    let silent = Bytes::from(silent_stereo_frame());

    let start = Instant::now();
    let plan: Vec<bool> = segments
        .iter()
        .flat_map(|(loud, n)| std::iter::repeat_n(*loud, *n))
        .collect();
    let total = plan.len();
    let mut timeline = Vec::new();
    for i in 0..total {
        if let Some(s) = stall {
            if i == s.after_frames {
                tokio::time::sleep(Duration::from_millis(s.stall_ms)).await;
            }
        }
        let due = start + Duration::from_millis(i as u64 * FRAME_MS);
        tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
        let frame = if plan[i] { &loud } else { &silent };
        agent.process_inbound_pcm(frame.clone(), 20).await.unwrap();
        while let Ok(event) = events.try_recv() {
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
    }
    // Let the wall-clock ticker run once more after the last frame.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(event) = events.try_recv() {
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
    let finalize_calls = script.lock().unwrap().finalize_calls;
    let _ = agent.stop().await;
    Observed {
        finals: timeline
            .iter()
            .filter(|(_, k, _)| *k == "final")
            .map(|(_, _, t)| t.clone())
            .collect(),
        partial_count: timeline.iter().filter(|(_, k, _)| *k == "partial").count(),
        finalize_calls,
        timeline,
    }
}

fn expected_final() -> Vec<String> {
    vec![words((SPEECH_FRAMES as u32 * FRAME_MS as u32) / 500)]
}

#[tokio::test]
async fn realtime_baseline_emits_one_final_with_every_word() {
    let observed = run(None, |_| {}).await;
    assert_eq!(
        observed.finals,
        expected_final(),
        "baseline timeline: {:?}",
        observed.timeline
    );
}

#[tokio::test]
async fn late_burst_in_speech_matches_realtime_finals() {
    let baseline = run(None, |_| {}).await;
    let burst = run(
        Some(Stall {
            after_frames: 50, // 1 s of speech
            stall_ms: 2000,
        }),
        |_| {},
    )
    .await;
    assert_eq!(
        burst.finals, baseline.finals,
        "burst timeline: {:?}\nbaseline timeline: {:?}",
        burst.timeline, baseline.timeline
    );
}

/// Stall DURING the end-of-turn silence at several offsets (frames of silence delivered before the
/// stall): during the VAD min-silence, during the gate hold, around hold expiry and during the
/// wait for the final. The final must equal the real-time one, once.
async fn silence_stall_matches_realtime(
    silence_frames_before_stall: usize,
    final_delay_polls: u32,
) {
    let baseline = run(None, |s| s.final_delay_polls = final_delay_polls).await;
    let stalled = run(
        Some(Stall {
            after_frames: SPEECH_FRAMES + silence_frames_before_stall,
            stall_ms: 2000,
        }),
        |s| s.final_delay_polls = final_delay_polls,
    )
    .await;
    assert_eq!(
        stalled.finals, baseline.finals,
        "stalled timeline: {:?}\nbaseline timeline: {:?}",
        stalled.timeline, baseline.timeline
    );
    assert_eq!(
        stalled.finalize_calls, baseline.finalize_calls,
        "vendor finalize calls differ; stalled timeline: {:?}",
        stalled.timeline
    );
}

#[tokio::test]
async fn stall_in_min_silence_matches_realtime_finals() {
    silence_stall_matches_realtime(20, 0).await;
}

#[tokio::test]
async fn stall_in_gate_hold_matches_realtime_finals() {
    silence_stall_matches_realtime(90, 0).await;
}

/// The final arrives 40 polls (about 0.8 s of media) after finalize: C2 (1500 ms) is armed and
/// waiting when the stall hits.
#[tokio::test]
async fn stall_while_waiting_for_slow_final_matches_realtime_finals() {
    silence_stall_matches_realtime(125, 40).await;
}

#[tokio::test]
async fn stall_while_waiting_for_slow_final_early_matches_realtime_finals() {
    silence_stall_matches_realtime(118, 40).await;
}

// fails before the fix: the wall-clock C2 ticker force-closes while the inbound call is still in
// its endpoint tail / finalize (armed_at and last_inbound_pcm_at are both older than 1500 ms), so
// finalize runs twice and a second, empty user_speech_final follows the real one.
/// Decode starved inside the inbound call: each 100 ms endpoint-tail chunk takes 400 ms, so one
/// `process_inbound_pcm` call runs for about 2.4 s while the wall-clock ticker keeps running.
/// Same audio, same arrival; only the STT is slow. The finals must still equal the fast run.
#[tokio::test]
async fn slow_decode_inside_finalize_matches_fast_decode_finals() {
    let fast = run(None, |_| {}).await;
    let slow = run(None, |s| s.tail_push_delay_ms = 400).await;
    assert_eq!(
        slow.finals, fast.finals,
        "slow timeline: {:?}\nfast timeline: {:?}",
        slow.timeline, fast.timeline
    );
    assert_eq!(
        slow.finalize_calls, fast.finalize_calls,
        "vendor finalize calls differ; slow timeline: {:?}",
        slow.timeline
    );
}

// fails before the fix: same cause as `slow_decode_inside_finalize_matches_fast_decode_finals`
// (an empty duplicate final after each utterance); the next utterance itself is fine.
/// After a slow finalize the next utterance must still reach the user: a second utterance of 2 s
/// of speech (4 words) follows the first one and must end in its own final.
#[tokio::test]
async fn second_utterance_after_slow_finalize_still_produces_a_final() {
    let segments = [
        (true, SPEECH_FRAMES),
        (false, SILENCE_FRAMES),
        (true, 100),
        (false, SILENCE_FRAMES),
    ];
    let observed = run_segments(&segments, None, |s| s.tail_push_delay_ms = 400).await;
    assert_eq!(
        observed.finals,
        vec![words(6), words(4)],
        "timeline: {:?}",
        observed.timeline
    );
}

// fails before the fix: C1 (`stt_listen_timeout_ms`, 4000 ms) runs on wall time from VAD
// SpeechStart. Only 120 ms of the utterance reached the agent before the stall, so no partial
// could exist yet; at 4 s of wall time `c2_wall_clock_tick` closes the STT stream
// (`close_stt_stream_not_found`) although the speech is only late, not absent. The rest of the
// utterance is then never sent to STT and no `user_speech_final` follows.
#[tokio::test]
async fn stall_before_first_partial_matches_realtime_finals() {
    let baseline = run(None, |_| {}).await;
    let stalled = run(
        Some(Stall {
            after_frames: 6, // 120 ms of speech: SpeechStart seen, no partial yet
            stall_ms: 4500,
        }),
        |_| {},
    )
    .await;
    assert_eq!(
        stalled.finals, baseline.finals,
        "stalled timeline: {:?}\nbaseline timeline: {:?}",
        stalled.timeline, baseline.timeline
    );
}
