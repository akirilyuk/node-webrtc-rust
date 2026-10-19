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

/// [`run_fullstack`] with the quiet time after the first terminal event set by the caller.
pub async fn run_fullstack_settling(
    factory: Arc<dyn VendorFactory>,
    stt: SttConfig,
    clip_stereo_48k: &[u8],
    expected: &[&str],
    settle: Duration,
) -> SpeechResult<FullstackSample> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(stt.provider, factory);
    let mut vad = preset_vad_config();
    vad.barge_in.enabled = false;
    let config = VoiceAgentConfig {
        stt: Some(stt),
        tts: None,
        vad,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry))?;
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await?;
    agent.start(None).await?;

    let clip = [
        silence_stereo_48k(1000),
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
    };
    let mut final_text = String::new();
    let mut first_terminal_at: Option<Instant> = None;
    let take = |sample: &mut FullstackSample,
                final_text: &mut String,
                first_terminal_at: &mut Option<Instant>,
                event: node_webrtc_rust_speech::events::SpeechEvent| {
        match event.kind {
            SpeechEventKind::UserSpeakingStart => sample.speech_started += 1,
            SpeechEventKind::UserSpeechPartial => sample.partials += 1,
            SpeechEventKind::UserSpeechFinal => {
                sample.finals += 1;
                final_text.push(' ');
                final_text.push_str(event.text.as_deref().unwrap_or(""));
                first_terminal_at.get_or_insert_with(Instant::now);
            }
            SpeechEventKind::UserSttNotFound => {
                sample.not_found += 1;
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

/// The full-stack contract for one spoken phrase: one utterance, exactly one
/// `user_speech_final`, no `user_stt_not_found`, no error event, and at least `min_words` of the
/// expected words in it. Prints `<label>_fullstack_finals` and `<label>_fullstack_words`.
pub fn assert_fullstack_contract(
    label: &str,
    sample: &FullstackSample,
    min_words: usize,
    total_words: usize,
) {
    println!("{label}_fullstack_finals {}", sample.finals);
    println!(
        "{label}_fullstack_words {}/{}",
        sample.matched_words, total_words
    );
    assert_eq!(
        sample.not_found, 0,
        "{label}: the agent closed the utterance as user_stt_not_found ({sample:?})"
    );
    assert_eq!(
        sample.finals, 1,
        "{label}: one spoken phrase produced {} user_speech_final events ({sample:?})",
        sample.finals
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
        }
    }

    #[test]
    fn contract_accepts_one_final() {
        assert_fullstack_contract("ok", &sample(), 8, 10);
    }

    #[test]
    #[should_panic(expected = "produced 2 user_speech_final")]
    fn contract_rejects_a_split_phrase() {
        let mut s = sample();
        s.finals = 2;
        assert_fullstack_contract("split", &s, 8, 10);
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
