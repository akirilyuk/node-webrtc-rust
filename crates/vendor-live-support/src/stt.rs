//! STT live measurement and the contract every cloud STT vendor must meet.
//!
//! Audio goes to the vendor the way the voice agent sends it: 20 ms frames of 16 kHz mono s16le
//! (`stereo_48k_to_mono_16k` of the 48 kHz stereo WebRTC frame), gated by the default energy VAD,
//! paced from an anchor. Transcripts are concatenated in memory and never printed.

use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{FinalizeMode, SttOutput, SttProvider, SttTranscript};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, sleep_until, timeout, Instant};

use crate::audio::matched_words;
use crate::vad_gate::{normalize_peak_stereo_48k, vad_gated_frames, CLIP_PEAK_DBFS};

/// One frame of the gated audio is 20 ms.
const FRAME: Duration = Duration::from_millis(20);
/// A push that ran this far behind schedule re-anchors the pacing instead of bursting.
const RE_ANCHOR_AFTER: Duration = Duration::from_millis(100);
/// Bound for the stream open (`wait_ready`).
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound for a single push or poll call.
const OP_TIMEOUT: Duration = Duration::from_secs(10);
/// Added to `finalize_wait_ms()` for the whole finalize.
const FINALIZE_SLACK_MS: u64 = 5000;

/// Timings and counts of one utterance. Contains no transcript text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SttSample {
    /// `start` to ready.
    pub open_ms: u64,
    /// First voiced frame pushed to the first partial. `None` when no partial arrived before
    /// the finalize.
    pub first_partial_ms: Option<u64>,
    /// Last voiced frame pushed to the last final.
    pub speech_end_to_final_ms: u64,
    /// Finalize requested to the last final.
    pub finalize_to_final_ms: u64,
    pub partials_before_finalize: usize,
    pub matched_words: usize,
    /// Audio actually streamed to the vendor.
    pub pushed_ms: u32,
    /// Length of the whole clip (including the padding that is not streamed).
    pub total_ms: u32,
}

#[derive(Default)]
struct Collector {
    transcript: String,
    partials_before_finalize: usize,
    first_partial_at: Option<Instant>,
    last_final_at: Option<Instant>,
    finalizing: bool,
}

impl Collector {
    fn take(&mut self, transcript: SttTranscript) {
        let now = Instant::now();
        match transcript {
            SttTranscript::Partial(_) => {
                if !self.finalizing {
                    self.partials_before_finalize += 1;
                    self.first_partial_at.get_or_insert(now);
                }
            }
            SttTranscript::Final(text) => {
                self.transcript.push(' ');
                self.transcript.push_str(&text);
                self.last_final_at = Some(now);
            }
        }
    }
}

async fn bounded<T>(
    vendor: &str,
    what: &str,
    limit: Duration,
    fut: impl Future<Output = SpeechResult<T>>,
) -> SpeechResult<T> {
    match timeout(limit, fut).await {
        Ok(result) => result,
        Err(_) => Err(SpeechError::Stt(format!(
            "{vendor}: {what} did not finish within {} ms",
            limit.as_millis()
        ))),
    }
}

fn is_voiced(frame: &Bytes) -> bool {
    frame
        .as_chunks::<2>()
        .0
        .iter()
        .any(|s| i16::from_le_bytes(*s) != 0)
}

/// Opens the stream (`start` + `wait_ready`) and returns the open time in ms.
pub async fn open_stt(stt: &mut dyn SttProvider) -> SpeechResult<u64> {
    let vendor = stt.vendor_name();
    let opened = Instant::now();
    bounded(vendor, "start", READY_TIMEOUT, stt.start()).await?;
    let ready = bounded(vendor, "wait_ready", READY_TIMEOUT, async {
        stt.wait_ready(READY_TIMEOUT).await
    })
    .await?;
    if !ready {
        return Err(SpeechError::Stt(format!(
            "{vendor}: stream not ready within {} s",
            READY_TIMEOUT.as_secs()
        )));
    }
    Ok(opened.elapsed().as_millis() as u64)
}

/// Streams one VAD-gated utterance to an **already opened** provider and finalizes it.
/// `open_ms` of the result is 0; [`measure_stt`] fills it in.
pub async fn run_utterance(
    stt: &mut dyn SttProvider,
    stereo_48k: &[u8],
    expected: &[&str],
) -> SpeechResult<SttSample> {
    let vendor = stt.vendor_name();
    let mode = stt.finalize_mode();
    // Vendor TTS can be quieter than the production energy VAD threshold: see the helper.
    let gated = vad_gated_frames(&normalize_peak_stereo_48k(stereo_48k, CLIP_PEAK_DBFS));
    let first_voiced = gated.frames_16k.iter().position(is_voiced);
    let last_voiced = gated.frames_16k.iter().rposition(is_voiced);
    let (Some(first_voiced), Some(last_voiced)) = (first_voiced, last_voiced) else {
        return Err(SpeechError::Stt(format!(
            "{vendor}: the VAD found no speech in the clip"
        )));
    };

    let mut collected = Collector::default();
    let mut anchor = Instant::now();
    let mut speech_start_at = None;
    let mut speech_end_at = None;

    // Frame n is due at anchor + n x 20 ms: no bursts, no drift.
    for (n, frame) in gated.frames_16k.iter().enumerate() {
        let due = anchor + FRAME * n as u32;
        sleep_until(due).await;
        let now = Instant::now();
        if now.saturating_duration_since(due) > RE_ANCHOR_AFTER {
            anchor = now - FRAME * n as u32;
        }
        if n == first_voiced {
            speech_start_at = Some(now);
        }
        if n == last_voiced {
            speech_end_at = Some(now);
        }
        match mode {
            FinalizeMode::Inline => {
                let mut out = Vec::new();
                bounded(
                    vendor,
                    "push_and_poll",
                    OP_TIMEOUT,
                    stt.push_and_poll(frame.clone(), &mut out),
                )
                .await?;
                for transcript in out {
                    collected.take(transcript);
                }
            }
            FinalizeMode::Deferred => {
                bounded(
                    vendor,
                    "push_audio",
                    OP_TIMEOUT,
                    stt.push_audio(frame.clone()),
                )
                .await?;
                while let Some(output) =
                    bounded(vendor, "poll_output", OP_TIMEOUT, stt.poll_output()).await?
                {
                    if let SttOutput::Transcript(transcript) = output {
                        collected.take(transcript);
                    }
                }
            }
        }
    }

    collected.finalizing = true;
    let finalize_at = Instant::now();
    let bound = Duration::from_millis(stt.finalize_wait_ms() + FINALIZE_SLACK_MS);
    let deadline = finalize_at + bound;
    let timed_out = || {
        SpeechError::Stt(format!(
            "{vendor}: no final within {} ms of the finalize",
            bound.as_millis()
        ))
    };
    let done_at;
    match mode {
        FinalizeMode::Deferred => {
            bounded(vendor, "request_finalize", bound, stt.request_finalize()).await?;
            loop {
                if Instant::now() >= deadline {
                    return Err(timed_out());
                }
                match bounded(vendor, "poll_output", OP_TIMEOUT, stt.poll_output()).await? {
                    Some(SttOutput::Transcript(transcript)) => collected.take(transcript),
                    Some(SttOutput::FinalizeDone) => {
                        done_at = Instant::now();
                        break;
                    }
                    None => sleep(Duration::from_millis(5)).await,
                }
            }
        }
        FinalizeMode::Inline => {
            bounded(
                vendor,
                "finalize_utterance",
                bound,
                stt.finalize_utterance(),
            )
            .await?;
            loop {
                match bounded(vendor, "poll_transcript", OP_TIMEOUT, stt.poll_transcript()).await? {
                    Some(transcript) => collected.take(transcript),
                    None if collected.last_final_at.is_some() => {
                        done_at = Instant::now();
                        break;
                    }
                    None => {
                        if Instant::now() >= deadline {
                            return Err(timed_out());
                        }
                        sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        }
    }

    let final_at = collected.last_final_at.unwrap_or(done_at);
    let speech_start_at = speech_start_at.expect("first voiced frame was pushed");
    let speech_end_at = speech_end_at.expect("last voiced frame was pushed");
    Ok(SttSample {
        open_ms: 0,
        first_partial_ms: collected
            .first_partial_at
            .map(|at| at.saturating_duration_since(speech_start_at).as_millis() as u64),
        speech_end_to_final_ms: final_at
            .saturating_duration_since(speech_end_at)
            .as_millis() as u64,
        finalize_to_final_ms: final_at.saturating_duration_since(finalize_at).as_millis() as u64,
        partials_before_finalize: collected.partials_before_finalize,
        matched_words: matched_words(&collected.transcript, expected),
        pushed_ms: gated.pushed_ms,
        total_ms: gated.total_ms,
    })
}

/// Opens the provider, streams the gated clip, finalizes, and returns the timings. The caller
/// stops the provider.
pub async fn measure_stt(
    stt: &mut dyn SttProvider,
    stereo_48k: &[u8],
    expected: &[&str],
) -> SpeechResult<SttSample> {
    let open_ms = open_stt(stt).await?;
    let mut sample = run_utterance(stt, stereo_48k, expected).await?;
    sample.open_ms = open_ms;
    Ok(sample)
}

/// The STT contract: at least 7 of the 9 phrase words, the silence padding not streamed, and
/// `emits_partials()` telling the truth. Prints `<label>_stt_final_ms` and
/// `<label>_stt_pushed_ms`.
pub fn assert_stt_contract(label: &str, provider_emits_partials: bool, sample: &SttSample) {
    println!("{label}_stt_final_ms {}", sample.speech_end_to_final_ms);
    println!(
        "{label}_stt_pushed_ms {}/{}",
        sample.pushed_ms, sample.total_ms
    );
    assert!(
        sample.matched_words >= 7,
        "{label}: matched {} of 9 words (transcript not printed)",
        sample.matched_words
    );
    assert!(
        sample.pushed_ms < sample.total_ms.saturating_sub(1000),
        "{label}: the silence padding was streamed: {} of {} ms",
        sample.pushed_ms,
        sample.total_ms
    );
    let saw_partials = sample.partials_before_finalize > 0;
    assert_eq!(
        provider_emits_partials, saw_partials,
        "{label}: emits_partials() is {provider_emits_partials} but {} partials arrived before the finalize",
        sample.partials_before_finalize
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad_gate::{silence_stereo_48k, tone_stereo_48k};
    use async_trait::async_trait;
    use std::collections::VecDeque;

    /// Emits a partial after `partial_after` pushes (if set), and the final on finalize.
    struct FakeStt {
        mode: FinalizeMode,
        partial_after: Option<usize>,
        pushes: usize,
        queue: VecDeque<SttOutput>,
        pushed_bytes: usize,
    }

    impl FakeStt {
        fn new(mode: FinalizeMode, partial_after: Option<usize>) -> Self {
            Self {
                mode,
                partial_after,
                pushes: 0,
                queue: VecDeque::new(),
                pushed_bytes: 0,
            }
        }

        fn on_push(&mut self, pcm: &Bytes) {
            self.pushes += 1;
            self.pushed_bytes += pcm.len();
            assert_eq!(pcm.len(), 640, "frames are 20 ms of 16 kHz mono s16le");
            if self.partial_after == Some(self.pushes) {
                self.queue
                    .push_back(SttOutput::Transcript(SttTranscript::Partial("the".into())));
            }
        }

        fn on_finalize(&mut self) {
            self.queue
                .push_back(SttOutput::Transcript(SttTranscript::Final(
                    "The quick, brown.".into(),
                )));
            if self.mode == FinalizeMode::Deferred {
                self.queue.push_back(SttOutput::FinalizeDone);
            }
        }
    }

    #[async_trait]
    impl SttProvider for FakeStt {
        fn vendor_name(&self) -> &'static str {
            "fake"
        }
        async fn start(&mut self) -> SpeechResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> SpeechResult<()> {
            Ok(())
        }
        async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
            self.on_push(&pcm);
            Ok(())
        }
        async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
            match self.queue.pop_front() {
                Some(SttOutput::Transcript(t)) => Ok(Some(t)),
                Some(SttOutput::FinalizeDone) | None => Ok(None),
            }
        }
        async fn finalize_utterance(&mut self) -> SpeechResult<()> {
            self.on_finalize();
            Ok(())
        }
        fn finalize_mode(&self) -> FinalizeMode {
            self.mode
        }
        async fn request_finalize(&mut self) -> SpeechResult<()> {
            self.on_finalize();
            Ok(())
        }
        async fn poll_output(&mut self) -> SpeechResult<Option<SttOutput>> {
            Ok(self.queue.pop_front())
        }
    }

    fn clip() -> Vec<u8> {
        [
            silence_stereo_48k(1000),
            tone_stereo_48k(1000, -12.0),
            silence_stereo_48k(2000),
        ]
        .concat()
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_provider_is_measured_and_paced() {
        let mut stt = FakeStt::new(FinalizeMode::Deferred, Some(10));
        let sample = measure_stt(&mut stt, &clip(), &["the", "quick", "brown"])
            .await
            .unwrap();
        assert_eq!(sample.matched_words, 3);
        assert_eq!(sample.partials_before_finalize, 1);
        assert!(sample.first_partial_ms.is_some());
        assert!(sample.pushed_ms < sample.total_ms - 1000);
        assert_eq!(stt.pushed_bytes, sample.pushed_ms as usize / 20 * 640);
        assert_stt_contract_for_words(&sample, true);
    }

    #[tokio::test(start_paused = true)]
    async fn inline_provider_without_partials_is_measured() {
        let mut stt = FakeStt::new(FinalizeMode::Inline, None);
        let sample = measure_stt(&mut stt, &clip(), &["the", "quick", "brown"])
            .await
            .unwrap();
        assert_eq!(sample.matched_words, 3);
        assert_eq!(sample.partials_before_finalize, 0);
        assert_eq!(sample.first_partial_ms, None);
        assert_stt_contract_for_words(&sample, false);
    }

    /// The real contract wants 7 words; the fake says 3, so lift the count for this check.
    fn assert_stt_contract_for_words(sample: &SttSample, emits_partials: bool) {
        let mut lifted = sample.clone();
        lifted.matched_words = 9;
        assert_stt_contract("fake", emits_partials, &lifted);
    }

    /// A clip at the level of Deepgram Aura speech (frame RMS ~0.1, below the VAD threshold 0.15)
    /// still streams: the clip is brought to -1 dBFS peak before gating.
    #[tokio::test(start_paused = true)]
    async fn quiet_vendor_speech_is_levelled_and_streamed() {
        let quiet = [
            silence_stereo_48k(1000),
            tone_stereo_48k(1500, -14.0),
            silence_stereo_48k(2000),
        ]
        .concat();
        let mut stt = FakeStt::new(FinalizeMode::Deferred, None);
        let sample = measure_stt(&mut stt, &quiet, &["the"]).await.unwrap();
        assert!(sample.pushed_ms > 0 && sample.pushed_ms < sample.total_ms - 1000);
    }

    #[tokio::test(start_paused = true)]
    async fn silence_only_is_an_error() {
        let mut stt = FakeStt::new(FinalizeMode::Deferred, None);
        let err = measure_stt(&mut stt, &silence_stereo_48k(3000), &["the"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no speech"), "{err}");
    }

    fn good() -> SttSample {
        SttSample {
            open_ms: 100,
            first_partial_ms: Some(300),
            speech_end_to_final_ms: 500,
            finalize_to_final_ms: 200,
            partials_before_finalize: 4,
            matched_words: 9,
            pushed_ms: 4500,
            total_ms: 6000,
        }
    }

    #[test]
    fn contract_accepts_a_good_sample() {
        assert_stt_contract("good", true, &good());
    }

    #[test]
    #[should_panic(expected = "matched 5 of 9")]
    fn contract_rejects_a_wrong_transcript() {
        let mut s = good();
        s.matched_words = 5;
        assert_stt_contract("bad", true, &s);
    }

    #[test]
    #[should_panic(expected = "silence padding was streamed")]
    fn contract_rejects_streamed_padding() {
        let mut s = good();
        s.pushed_ms = 5500;
        assert_stt_contract("padded", true, &s);
    }

    #[test]
    #[should_panic(expected = "emits_partials() is true but 0 partials")]
    fn contract_rejects_a_provider_that_claims_partials_but_sends_none() {
        let mut s = good();
        s.partials_before_finalize = 0;
        assert_stt_contract("liar", true, &s);
    }

    #[test]
    #[should_panic(expected = "emits_partials() is false but 4 partials")]
    fn contract_rejects_a_provider_that_denies_partials_but_sends_them() {
        assert_stt_contract("liar", false, &good());
    }
}
