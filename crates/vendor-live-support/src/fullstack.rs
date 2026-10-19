//! Full-stack STT check: the real [`VoiceAgent`] and its STT actor with the production VAD preset
//! (`gate_stt: true`), fed the way the runner feeds it, instead of the direct provider calls of
//! [`crate::stt::measure_stt`].
//!
//! What it adds over `measure_stt`: the agent decides when the utterance ends (VAD hangover, gate
//! hold, `EndUtterance`), the actor attributes transcripts to that utterance, and the agent turns
//! them into `user_speech_final` / `user_stt_not_found`. A provider that splits a phrase into
//! several finals, answers its finalize late, or leaks a late segment into the next utterance
//! shows up as a wrong event count here and nowhere else.
//!
//! Audio is pushed as inbound 20 ms 48 kHz stereo frames paced from an anchor. Transcripts are
//! reduced to counts; nothing is printed.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, VoiceAgentConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use tokio::time::{sleep, sleep_until, Instant};

use crate::audio::matched_words;
use crate::vad_gate::{
    normalize_peak_stereo_48k, preset_vad_config, silence_stereo_48k, CLIP_PEAK_DBFS, FRAME_BYTES,
};

const FRAME: Duration = Duration::from_millis(20);
/// A push that ran this far behind schedule re-anchors the pacing instead of bursting.
const RE_ANCHOR_AFTER: Duration = Duration::from_millis(100);
/// Silence after the speech: longer than the VAD hangover (1300 ms) plus the gate hold (1000 ms)
/// plus the endpoint tail, so the agent ends the utterance on its own. Not streamed to the
/// vendor (the gate is closed).
pub const TAIL_SILENCE_MS: usize = 4000;
/// Longest wait, after the last frame, for the first terminal event of the utterance.
const TERMINAL_WAIT: Duration = Duration::from_secs(20);
/// Wait after the first terminal event for further events of the same utterance (a split final,
/// a late final): the agent has nothing left to send, so this is quiet time.
const SETTLE: Duration = Duration::from_secs(4);

/// Counts of what the agent emitted for the pushed clip. Contains no transcript text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullstackSample {
    pub speech_started: usize,
    /// `user_speech_final` events.
    pub finals: usize,
    /// `user_stt_not_found` events.
    pub not_found: usize,
    pub partials: usize,
    pub errors: usize,
    /// Expected words found in the concatenated final texts.
    pub matched_words: usize,
    /// Milliseconds from the first frame of the speech (after the leading silence) to each event
    /// kind's first occurrence; `None` when it never happened.
    pub first_partial_ms: Option<u64>,
    pub first_final_ms: Option<u64>,
    pub first_not_found_ms: Option<u64>,
}

/// Runs `clip_stereo_48k` (speech only, no padding) through a `VoiceAgent` whose STT is
/// `factory.create_stt(&stt)`. The clip is levelled like the vendor TTS fixtures of
/// [`crate::stt::run_utterance`], then padded with 1 s of silence before and
/// [`TAIL_SILENCE_MS`] after.
pub async fn run_fullstack(
    factory: Arc<dyn VendorFactory>,
    stt: SttConfig,
    clip_stereo_48k: &[u8],
    expected: &[&str],
) -> SpeechResult<FullstackSample> {
    run_fullstack_settling(factory, stt, clip_stereo_48k, expected, SETTLE).await
}

/// Frames of silence before the speech in every run; the timeline's zero is the first speech frame.
const LEAD_SILENCE_MS: usize = 1000;

/// [`run_fullstack`] with the quiet time after the first terminal event set by the caller.
pub async fn run_fullstack_settling(
    factory: Arc<dyn VendorFactory>,
    stt: SttConfig,
    clip_stereo_48k: &[u8],
    expected: &[&str],
    settle: Duration,
) -> SpeechResult<FullstackSample> {
    let mut vad = preset_vad_config();
    vad.barge_in.enabled = false;
    let config = VoiceAgentConfig {
        stt: Some(stt),
        tts: None,
        vad,
        ..Default::default()
    };
    run_fullstack_with_config(factory, config, clip_stereo_48k, expected, settle).await
}

/// [`run_fullstack_settling`] with the whole [`VoiceAgentConfig`] given by the caller, for example
/// [`crate::runner_config::runner_staging_config`]. The STT vendor is `config.stt`. When the config
/// enables language ID, the local Sherpa factory is registered for it (feature `sherpa-lid`); a
/// build without it panics instead of running without the LID the config asks for.
pub async fn run_fullstack_with_config(
    factory: Arc<dyn VendorFactory>,
    config: VoiceAgentConfig,
    clip_stereo_48k: &[u8],
    expected: &[&str],
    settle: Duration,
) -> SpeechResult<FullstackSample> {
    run_fullstack_idle_then_speak(
        factory,
        config,
        clip_stereo_48k,
        expected,
        settle,
        LEAD_SILENCE_MS,
    )
    .await
}

/// [`run_fullstack_with_config`] with `lead_silence_ms` of silence (gate closed, nothing sent to
/// the vendor) between the agent start and the speech: a session that sits idle before the caller
/// speaks, as every real session does while it connects, greets and waits.
pub async fn run_fullstack_idle_then_speak(
    factory: Arc<dyn VendorFactory>,
    config: VoiceAgentConfig,
    clip_stereo_48k: &[u8],
    expected: &[&str],
    settle: Duration,
    lead_silence_ms: usize,
) -> SpeechResult<FullstackSample> {
    let stt = config
        .stt
        .clone()
        .ok_or_else(|| SpeechError::Config("run_fullstack_with_config needs config.stt".into()))?;
    let mut registry = VendorRegistry::new();
    registry.register_stt(stt.provider, factory);
    if node_webrtc_rust_speech::config::language_id_enabled(&config.language_id) {
        crate::runner_config::register_lid(&mut registry);
    }
    let agent = VoiceAgent::new(config, Arc::new(registry))?;
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await?;
    agent.start(None).await?;

    let clip = [
        silence_stereo_48k(lead_silence_ms),
        normalize_peak_stereo_48k(clip_stereo_48k, CLIP_PEAK_DBFS),
        silence_stereo_48k(TAIL_SILENCE_MS),
    ]
    .concat();

    let mut sample = FullstackSample {
        speech_started: 0,
        finals: 0,
        not_found: 0,
        partials: 0,
        errors: 0,
        matched_words: 0,
        first_partial_ms: None,
        first_final_ms: None,
        first_not_found_ms: None,
    };
    let mut final_text = String::new();
    let mut first_terminal_at: Option<Instant> = None;
    let run_start = Instant::now();
    let speech_zero = run_start + Duration::from_millis(lead_silence_ms as u64);
    let since_speech = move || {
        Instant::now()
            .saturating_duration_since(speech_zero)
            .as_millis() as u64
    };
    let take = |sample: &mut FullstackSample,
                final_text: &mut String,
                first_terminal_at: &mut Option<Instant>,
                event: node_webrtc_rust_speech::events::SpeechEvent| {
        match event.kind {
            SpeechEventKind::UserSpeakingStart => sample.speech_started += 1,
            SpeechEventKind::UserSpeechPartial => {
                sample.partials += 1;
                sample.first_partial_ms.get_or_insert_with(since_speech);
            }
            SpeechEventKind::UserSpeechFinal => {
                sample.finals += 1;
                sample.first_final_ms.get_or_insert_with(since_speech);
                final_text.push(' ');
                final_text.push_str(event.text.as_deref().unwrap_or(""));
                first_terminal_at.get_or_insert_with(Instant::now);
            }
            SpeechEventKind::UserSttNotFound => {
                sample.not_found += 1;
                sample.first_not_found_ms.get_or_insert_with(since_speech);
                first_terminal_at.get_or_insert_with(Instant::now);
            }
            SpeechEventKind::Error => sample.errors += 1,
            _ => {}
        }
    };

    // Frame n is due at anchor + n x 20 ms: no bursts, no drift.
    let mut anchor = Instant::now();
    for (n, frame) in clip.chunks(FRAME_BYTES).enumerate() {
        let due = anchor + FRAME * n as u32;
        sleep_until(due).await;
        let now = Instant::now();
        if now.saturating_duration_since(due) > RE_ANCHOR_AFTER {
            anchor = now - FRAME * n as u32;
        }
        agent
            .process_inbound_pcm(Bytes::from(frame.to_vec()), 20)
            .await?;
        while let Ok(event) = events.try_recv() {
            take(&mut sample, &mut final_text, &mut first_terminal_at, event);
        }
    }

    // The first terminal event, then quiet time for anything more.
    let wait_until = Instant::now() + TERMINAL_WAIT;
    while first_terminal_at.is_none() && Instant::now() < wait_until {
        sleep(Duration::from_millis(20)).await;
        while let Ok(event) = events.try_recv() {
            take(&mut sample, &mut final_text, &mut first_terminal_at, event);
        }
    }
    let settle_until = Instant::now() + settle;
    while Instant::now() < settle_until {
        sleep(Duration::from_millis(20)).await;
        while let Ok(event) = events.try_recv() {
            take(&mut sample, &mut final_text, &mut first_terminal_at, event);
        }
    }
    if !agent.wait_stt_idle(Duration::from_secs(10)).await {
        return Err(SpeechError::Stt("the STT actor did not go idle".into()));
    }
    agent.stop().await?;
    while let Ok(event) = events.try_recv() {
        take(&mut sample, &mut final_text, &mut first_terminal_at, event);
    }
    sample.matched_words = matched_words(&final_text, expected);
    Ok(sample)
}

/// The full-stack contract for one spoken phrase: exactly one `user_speech_final` per
/// `user_speaking_start` (per VAD utterance), no `user_stt_not_found`, no error event, and at least `min_words` of the
/// expected words across all finals. Prints `<label>_fullstack_finals` and `<label>_fullstack_words`.
pub fn assert_fullstack_contract(
    label: &str,
    sample: &FullstackSample,
    min_words: usize,
    total_words: usize,
) {
    println!("{label}_fullstack_finals {}", sample.finals);
    println!(
        "{label}_fullstack_first_partial_ms {:?} first_final_ms {:?} first_not_found_ms {:?}",
        sample.first_partial_ms, sample.first_final_ms, sample.first_not_found_ms
    );
    println!(
        "{label}_fullstack_words {}/{}",
        sample.matched_words, total_words
    );
    assert_eq!(
        sample.not_found, 0,
        "{label}: the agent closed the utterance as user_stt_not_found ({sample:?})"
    );
    // The VAD may split a phrase at a long pause into several utterances (`user_speaking_start`);
    // each one gets exactly one final.
    assert!(
        sample.speech_started >= 1,
        "{label}: no utterance started ({sample:?})"
    );
    assert_eq!(
        sample.finals, sample.speech_started,
        "{label}: {} utterances produced {} user_speech_final events ({sample:?})",
        sample.speech_started, sample.finals
    );
    assert_eq!(sample.errors, 0, "{label}: error events ({sample:?})");
    assert!(
        sample.matched_words >= min_words,
        "{label}: matched {} of {total_words} words, needed {min_words} (transcript not printed)",
        sample.matched_words
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FullstackSample {
        FullstackSample {
            speech_started: 1,
            finals: 1,
            not_found: 0,
            partials: 3,
            errors: 0,
            matched_words: 9,
            first_partial_ms: Some(700),
            first_final_ms: Some(6000),
            first_not_found_ms: None,
        }
    }

    #[test]
    fn contract_accepts_one_final() {
        assert_fullstack_contract("ok", &sample(), 8, 10);
    }

    #[test]
    #[should_panic(expected = "produced 2 user_speech_final")]
    fn contract_rejects_two_finals_for_one_utterance() {
        let mut s = sample();
        s.finals = 2;
        assert_fullstack_contract("split", &s, 8, 10);
    }

    #[test]
    fn contract_accepts_one_final_per_utterance_when_the_vad_splits_the_phrase() {
        let mut s = sample();
        s.speech_started = 2;
        s.finals = 2;
        assert_fullstack_contract("two", &s, 8, 10);
    }

    #[test]
    #[should_panic(expected = "2 utterances produced 1 user_speech_final")]
    fn contract_rejects_an_utterance_without_a_final() {
        let mut s = sample();
        s.speech_started = 2;
        assert_fullstack_contract("lost", &s, 8, 10);
    }

    #[test]
    #[should_panic(expected = "user_stt_not_found")]
    fn contract_rejects_not_found() {
        let mut s = sample();
        s.finals = 0;
        s.not_found = 1;
        assert_fullstack_contract("nf", &s, 8, 10);
    }

    #[test]
    #[should_panic(expected = "matched 5 of 10")]
    fn contract_rejects_missing_words() {
        let mut s = sample();
        s.matched_words = 5;
        assert_fullstack_contract("few", &s, 8, 10);
    }
}
