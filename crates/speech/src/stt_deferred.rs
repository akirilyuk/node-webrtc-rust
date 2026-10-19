//! Output queue of a [`FinalizeMode::Deferred`](crate::pipeline::FinalizeMode) STT provider whose
//! vendor streams several final segments per utterance (Deepgram `is_final`, ElevenLabs
//! `committed_transcript`) and only knows the end of the utterance when the agent says so.
//!
//! The contract (see [`SttOutput::FinalizeDone`]): one utterance is one
//! [`SttTranscript::Final`], emitted when the vendor answers the agent's finalize request, and
//! followed by exactly one [`SttOutput::FinalizeDone`] per [`DeferredFinals::request`].
//!
//! - Interim and segment text is shown as [`SttTranscript::Partial`] (the segments so far plus
//!   the interim tail), never as a final.
//! - [`DeferredFinals::answer`] closes the oldest pending finalize: the joined segments (when
//!   non-empty) as one `Final`, then the marker.
//! - A finalize the vendor never answers is closed by [`DeferredFinals::expire`] after the
//!   provider's wait bound, flushing what it has, so the marker count stays aligned with the
//!   requests and the agent's actor never swallows the wrong marker.
//! - A lost connection answers every pending finalize ([`DeferredFinals::lost`]).
//!
//! Transcript text is never logged or put into an error message.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::error::{SpeechError, SpeechResult};
use crate::pipeline::{SttOutput, SttTranscript};

enum Item {
    Output(SttOutput),
    /// A vendor error, surfaced as `Err` from the next poll.
    Failure(String),
}

#[derive(Default)]
pub struct DeferredFinals {
    queue: VecDeque<Item>,
    /// Finalizes requested and not yet answered, oldest first (request time).
    pending: VecDeque<Instant>,
    /// Final segments of the utterance in progress, joined by single spaces.
    segments: String,
}

impl DeferredFinals {
    pub fn new() -> Self {
        Self::default()
    }

    /// Finalizes still waiting for the vendor.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// An utterance is in progress (final segments received and not yet delivered).
    pub fn has_segments(&self) -> bool {
        !self.segments.is_empty()
    }

    /// Interim text of the segment being spoken: a partial of segments + interim.
    pub fn interim(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let partial = Self::join(&self.segments, text);
        self.queue
            .push_back(Item::Output(SttOutput::Transcript(SttTranscript::Partial(
                partial,
            ))));
    }

    /// A final segment of the utterance in progress. Shown as a partial; delivered as part of
    /// the one final.
    pub fn segment(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.segments = Self::join(&self.segments, text);
        self.queue
            .push_back(Item::Output(SttOutput::Transcript(SttTranscript::Partial(
                self.segments.clone(),
            ))));
    }

    /// The agent asked for the end of the utterance.
    pub fn request(&mut self, now: Instant) {
        self.pending.push_back(now);
    }

    /// The vendor answered the oldest pending finalize. Returns false (and does nothing) when
    /// none is pending.
    pub fn answer(&mut self) -> bool {
        if self.pending.pop_front().is_none() {
            return false;
        }
        self.flush();
        self.queue.push_back(Item::Output(SttOutput::FinalizeDone));
        true
    }

    /// The vendor failed the oldest pending finalize: the error, then the closing marker.
    pub fn fail(&mut self, message: String) {
        self.queue.push_back(Item::Failure(message));
        if self.pending.pop_front().is_some() {
            self.flush();
            self.queue.push_back(Item::Output(SttOutput::FinalizeDone));
        }
    }

    /// A vendor error that is not the answer to a finalize: surfaced, pending stays pending.
    pub fn fail_without_answer(&mut self, message: String) {
        self.queue.push_back(Item::Failure(message));
    }

    /// Closes every finalize older than `wait` that the vendor never answered.
    pub fn expire(&mut self, now: Instant, wait: Duration) {
        while self
            .pending
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= wait)
        {
            self.answer();
        }
    }

    /// The connection ended. Something was lost when a finalize is pending or segments are
    /// waiting: one error, the text so far as a final for the oldest pending finalize, then one
    /// marker per pending finalize. An idle close is silent.
    pub fn lost(&mut self, message: String) {
        if self.pending.is_empty() && self.segments.is_empty() {
            return;
        }
        self.queue.push_back(Item::Failure(message));
        if self.pending.is_empty() {
            // Text of an utterance whose end never came: nothing to attribute it to.
            self.segments.clear();
            return;
        }
        while self.answer() {}
    }

    /// Drops queued transcripts (markers stay). Returns how many were dropped.
    pub fn discard_transcripts(&mut self) -> usize {
        let before = self.queue.len();
        self.queue
            .retain(|item| !matches!(item, Item::Output(SttOutput::Transcript(_))));
        before - self.queue.len()
    }

    /// Forget everything (provider start / stop).
    pub fn clear(&mut self) {
        self.queue.clear();
        self.pending.clear();
        self.segments.clear();
    }

    pub fn next_output(&mut self) -> SpeechResult<Option<SttOutput>> {
        match self.queue.pop_front() {
            None => Ok(None),
            Some(Item::Failure(message)) => Err(SpeechError::Stt(message)),
            Some(Item::Output(output)) => Ok(Some(output)),
        }
    }

    /// Next transcript; markers are for [`Self::next_output`] callers only.
    pub fn next_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        loop {
            match self.queue.pop_front() {
                None => return Ok(None),
                Some(Item::Failure(message)) => return Err(SpeechError::Stt(message)),
                Some(Item::Output(SttOutput::Transcript(t))) => return Ok(Some(t)),
                Some(Item::Output(SttOutput::FinalizeDone)) => {}
            }
        }
    }

    fn flush(&mut self) {
        if !self.segments.is_empty() {
            let text = std::mem::take(&mut self.segments);
            self.queue
                .push_back(Item::Output(SttOutput::Transcript(SttTranscript::Final(
                    text,
                ))));
        }
    }

    fn join(head: &str, tail: &str) -> String {
        if head.is_empty() {
            tail.to_string()
        } else {
            format!("{head} {tail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(d: &mut DeferredFinals) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(Some(o)) = d.next_output() {
            out.push(match o {
                SttOutput::Transcript(SttTranscript::Partial(t)) => format!("p:{t}"),
                SttOutput::Transcript(SttTranscript::Final(t)) => format!("f:{t}"),
                SttOutput::FinalizeDone => "done".into(),
            });
        }
        out
    }

    #[test]
    fn segments_are_one_final_at_the_answer() {
        let mut d = DeferredFinals::new();
        d.segment("one two");
        d.interim("three");
        d.segment("three four");
        d.request(Instant::now());
        assert!(d.answer());
        assert_eq!(
            drain(&mut d),
            [
                "p:one two",
                "p:one two three",
                "p:one two three four",
                "f:one two three four",
                "done"
            ]
        );
        assert!(!d.has_segments());
    }

    #[test]
    fn answer_without_text_is_only_the_marker() {
        let mut d = DeferredFinals::new();
        d.request(Instant::now());
        assert!(d.answer());
        assert_eq!(drain(&mut d), ["done"]);
        assert!(!d.answer());
    }

    #[test]
    fn expire_closes_only_old_finalizes_and_keeps_alignment() {
        let mut d = DeferredFinals::new();
        let t0 = Instant::now();
        d.segment("a");
        d.request(t0);
        d.request(t0 + Duration::from_secs(8));
        d.expire(t0 + Duration::from_secs(10), Duration::from_secs(10));
        assert_eq!(drain(&mut d), ["p:a", "f:a", "done"]);
        assert_eq!(d.pending(), 1);
    }

    #[test]
    fn lost_answers_every_pending_finalize_once() {
        let mut d = DeferredFinals::new();
        d.segment("a");
        d.request(Instant::now());
        d.request(Instant::now());
        d.lost("closed".into());
        d.lost("closed again".into());
        assert!(d.next_output().is_ok()); // partial
        assert!(d.next_output().is_err()); // the error
        assert_eq!(drain(&mut d), ["f:a", "done", "done"]);
    }

    #[test]
    fn idle_loss_is_quiet() {
        let mut d = DeferredFinals::new();
        d.lost("closed".into());
        assert_eq!(d.next_output().unwrap(), None);
    }

    #[test]
    fn poll_transcript_skips_markers() {
        let mut d = DeferredFinals::new();
        d.request(Instant::now());
        d.answer();
        d.segment("x");
        assert_eq!(
            d.next_transcript().unwrap(),
            Some(SttTranscript::Partial("x".into()))
        );
        assert_eq!(d.next_transcript().unwrap(), None);
    }
}
