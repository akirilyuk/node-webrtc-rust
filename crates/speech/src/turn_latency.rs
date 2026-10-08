//! Per-turn latency bookkeeping: how long a caller waits between finishing a sentence
//! (VAD `speech_end`) and hearing the first audio of the reply.
//!
//! Pure state machine over [`Instant`]s so the timing rules are unit-testable without
//! an agent. The agent feeds it VAD transitions, STT finals, barge-ins and the first
//! outbound PCM frame of each reply, and records the returned values as OTel metrics.

use std::time::Instant;

/// A turn that received an STT final but never got reply audio before the caller spoke again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Abandoned;

/// Timings produced when the first reply frame of a turn is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReplyTimings {
    /// Latest STT final to first outbound PCM frame.
    pub final_to_audio_ms: f64,
    /// VAD `speech_end` to first outbound PCM frame; `None` when VAD saw no `speech_end`.
    pub turn_response_ms: Option<f64>,
}

#[derive(Debug, Default)]
pub(crate) struct TurnLatencyTracker {
    speech_end_at: Option<Instant>,
    final_at: Option<Instant>,
    finalize_recorded: bool,
}

fn ms_between(later: Instant, earlier: Instant) -> f64 {
    later.saturating_duration_since(earlier).as_secs_f64() * 1000.0
}

impl TurnLatencyTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self) {
        self.speech_end_at = None;
        self.final_at = None;
        self.finalize_recorded = false;
    }

    fn abandon_if_answered_final(&mut self) -> Option<Abandoned> {
        if self.final_at.is_some() {
            self.reset();
            Some(Abandoned)
        } else {
            None
        }
    }

    /// VAD `speech_start`: the caller is talking again.
    pub(crate) fn on_speech_start(&mut self, _now: Instant) -> Option<Abandoned> {
        self.abandon_if_answered_final()
    }

    /// VAD `speech_end`: start a new pending turn.
    pub(crate) fn on_speech_end(&mut self, now: Instant) {
        self.speech_end_at = Some(now);
        self.final_at = None;
        self.finalize_recorded = false;
    }

    /// STT final. Returns finalize ms (`speech_end` to the first final of this utterance) once.
    pub(crate) fn on_stt_final(&mut self, now: Instant) -> Option<f64> {
        let finalize_ms = match self.speech_end_at {
            Some(end) if !self.finalize_recorded => {
                self.finalize_recorded = true;
                Some(ms_between(now, end))
            }
            _ => None,
        };
        self.final_at = Some(now);
        finalize_ms
    }

    /// First outbound PCM frame of a reply. Only counts when a final is pending.
    pub(crate) fn on_first_reply_audio(&mut self, now: Instant) -> Option<ReplyTimings> {
        let final_at = self.final_at?;
        let timings = ReplyTimings {
            final_to_audio_ms: ms_between(now, final_at),
            turn_response_ms: self.speech_end_at.map(|end| ms_between(now, end)),
        };
        self.reset();
        Some(timings)
    }

    /// Barge-in during agent playback.
    pub(crate) fn on_barge_in(&mut self, _now: Instant) -> Option<Abandoned> {
        self.abandon_if_answered_final()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn speech_end_final_audio_records_all_three() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        t.on_speech_end(t0);
        let finalize = t.on_stt_final(at(t0, 300)).expect("finalize");
        assert!((finalize - 300.0).abs() < 1e-6);
        let r = t.on_first_reply_audio(at(t0, 900)).expect("reply");
        assert!((r.final_to_audio_ms - 600.0).abs() < 1e-6);
        assert!((r.turn_response_ms.expect("turn") - 900.0).abs() < 1e-6);
    }

    #[test]
    fn two_finals_finalize_uses_first_final_to_audio_uses_latest() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        t.on_speech_end(t0);
        assert!((t.on_stt_final(at(t0, 200)).expect("first") - 200.0).abs() < 1e-6);
        assert!(t.on_stt_final(at(t0, 500)).is_none());
        let r = t.on_first_reply_audio(at(t0, 1000)).expect("reply");
        assert!((r.final_to_audio_ms - 500.0).abs() < 1e-6);
        assert!((r.turn_response_ms.expect("turn") - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn speech_start_after_final_before_audio_is_abandoned() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        t.on_speech_end(t0);
        t.on_stt_final(at(t0, 300));
        assert_eq!(t.on_speech_start(at(t0, 400)), Some(Abandoned));
        assert!(t.on_first_reply_audio(at(t0, 900)).is_none());
    }

    #[test]
    fn barge_in_after_final_is_abandoned() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        t.on_speech_end(t0);
        t.on_stt_final(at(t0, 300));
        assert_eq!(t.on_barge_in(at(t0, 400)), Some(Abandoned));
        assert!(t.on_first_reply_audio(at(t0, 900)).is_none());
    }

    #[test]
    fn speech_start_without_final_is_not_abandoned() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        t.on_speech_end(t0);
        assert_eq!(t.on_speech_start(at(t0, 100)), None);
        assert_eq!(t.on_barge_in(at(t0, 120)), None);
    }

    #[test]
    fn audio_with_no_pending_turn_is_none() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        assert!(t.on_first_reply_audio(t0).is_none());
    }

    #[test]
    fn vad_off_final_to_audio_only() {
        let t0 = Instant::now();
        let mut t = TurnLatencyTracker::new();
        assert!(t.on_stt_final(t0).is_none());
        let r = t.on_first_reply_audio(at(t0, 450)).expect("reply");
        assert!((r.final_to_audio_ms - 450.0).abs() < 1e-6);
        assert!(r.turn_response_ms.is_none());
    }
}
