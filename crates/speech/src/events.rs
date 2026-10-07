//! Speech event bus and event types.
//!
//! Events are emitted by [`crate::agent::VoiceAgent`] and broadcast on [`SpeechEventBus`].
//! TypeScript maps these to `SpeechEventType` strings in `@node-webrtc-rust/sdk/voice`.
//!
//! ## Event semantics (summary)
//!
//! | Kind | Typical trigger |
//! | ---- | ----------------- |
//! | `UserSpeakingStart` | VAD `SpeechStart` |
//! | `UserSpeakingEnd` | With `gate_stt` + STT: paired with final; else after hold or VAD end |
//! | `UserSpeechPartial` | STT streaming |
//! | `UserSpeechFinal` | STT `finalize_utterance` — primary turn boundary for LLM |
//! | `UserLanguage` | Offline spoken-language ID on buffered user PCM |
//! | `LanguageIdSkipped` | Utterance closed (or LID deferred for TTS) without an identify running; no `user_language` will follow unless a deferred identify later runs |
//! | `AgentSpeakingStart` | First TTS PCM frame queued to outbound; carries `first_chunk_ms` / `first_audio_ms` when timed |
//! | `AgentSpeakingEnd` | TTS queue drained — **only on the agent that plays TTS** |
//! | `VadTriggered` | VAD `SpeechStart` when `vad.enabled` — opens STT listen for this utterance |
//! | `SttStreamStart` / `SttStreamEnd` | STT vendor PCM feed opened / closed for an utterance |
//! | `UserSttStart` / `UserSttEnd` | STT recognition session for one user utterance |
//! | `UserSttNotFound` | VAD fired but no STT partial within `sttListenTimeoutMs` |
//! | `BargeIn` | Barge-in path (semantic STT partial and/or VAD during agent TTS) |
//! | `TtsWait` | A TTS synthesis start was refused and retried (`wait_ms`, `attempts`, `reason`) |
//! | `AgentSpeakFailed` | One reply could not be synthesized (`reason`); the session keeps running |
//! | `Error` | Vendor or internal failure |

use crate::config::{SttConfig, TtsConfig};

use tokio::sync::broadcast;

/// Speech lifecycle events emitted by the voice agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechEventKind {
    UserSpeakingStart,
    UserSpeakingEnd,
    UserSpeechPartial,
    UserSpeechFinal,
    UserLanguage,
    /// No language decision for this utterance (`reason`, `speech_ms`). No transcript text.
    LanguageIdSkipped,
    AgentSpeakingStart,
    AgentSpeakingEnd,
    VadTriggered,
    SttStreamStart,
    SttStreamEnd,
    UserSttStart,
    UserSttEnd,
    UserSttNotFound,
    BargeIn,
    Error,
    SttConfigUpdated,
    TtsConfigUpdated,
    /// `begin_stt_hold` accepted (`hold_mode`, `buffered_ms` seeded from the open utterance).
    SttHoldStarted,
    /// Hold finished (`hold_outcome`, `buffered_ms`, `dropped_ms`). No transcript text.
    SttHoldEnded,
    /// TTS synthesis start waited on a transient refusal (`wait_ms`, `attempts`, `reason`).
    TtsWait,
    /// One reply failed to synthesize (`reason`); the agent keeps listening.
    AgentSpeakFailed,
}

/// A speech event with optional payload text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechEvent {
    pub kind: SpeechEventKind,
    pub text: Option<String>,
    /// ISO 639-1 code on `user_language` events.
    pub language: Option<String>,
    pub error: Option<String>,
    /// Shared id for `user_speaking_start` … `user_speech_final` / `user_language` on one utterance.
    pub utterance_id: Option<String>,
    /// True when this `user_speech_final` was produced by `replay_last_utterance`.
    pub replay: Option<bool>,
    /// Original utterance id when `replay` is true.
    pub replaces_utterance_id: Option<String>,
    /// Runner may set on held finals released after a failed switch.
    pub language_mismatch: Option<bool>,
    /// TTS voice id on `tts_config_updated`.
    pub voice: Option<String>,
    pub model_path: Option<String>,
    pub endpoint: Option<String>,
    /// `stt_hold_started`: `buffer_replay` | `first_utterance`.
    pub hold_mode: Option<String>,
    /// `stt_hold_ended`: `released_replay` | `released_drop` | `cancelled`.
    pub hold_outcome: Option<String>,
    /// Held PCM duration (ms) on `stt_hold_*`.
    pub buffered_ms: Option<u32>,
    /// PCM duration (ms) dropped by the buffer bound or `first_utterance` on `stt_hold_ended`.
    pub dropped_ms: Option<u32>,
    /// `language_id_skipped`: `too_short` | `deferred_tts` | `no_audio` | `undetermined`.
    pub reason: Option<String>,
    /// `language_id_skipped`: buffered user speech (ms) at the decision point.
    pub speech_ms: Option<u32>,
    /// `agent_speaking_start`: ms from the speak request to the first PCM chunk from the TTS vendor.
    pub first_chunk_ms: Option<u32>,
    /// `agent_speaking_start`: ms from the speak request to the first outbound PCM frame of that reply.
    pub first_audio_ms: Option<u32>,
    /// `tts_wait`: ms the synthesis start waited on refusals.
    pub wait_ms: Option<u32>,
    /// `tts_wait`: refused attempts that were retried.
    pub attempts: Option<u32>,
}

impl SpeechEvent {
    fn base(kind: SpeechEventKind) -> Self {
        Self {
            kind,
            text: None,
            language: None,
            error: None,
            utterance_id: None,
            replay: None,
            replaces_utterance_id: None,
            language_mismatch: None,
            voice: None,
            model_path: None,
            endpoint: None,
            hold_mode: None,
            hold_outcome: None,
            buffered_ms: None,
            dropped_ms: None,
            reason: None,
            speech_ms: None,
            first_chunk_ms: None,
            first_audio_ms: None,
            wait_ms: None,
            attempts: None,
        }
    }

    /// A TTS synthesis start was refused and retried (`reason`: gRPC code + message).
    pub fn tts_wait(wait_ms: u32, attempts: u32, reason: impl Into<String>) -> Self {
        let mut ev = Self::base(SpeechEventKind::TtsWait);
        ev.wait_ms = Some(wait_ms);
        ev.attempts = Some(attempts);
        ev.reason = Some(reason.into());
        ev
    }

    /// One reply failed to synthesize; the session stays up.
    pub fn agent_speak_failed(reason: impl Into<String>) -> Self {
        let mut ev = Self::base(SpeechEventKind::AgentSpeakFailed);
        ev.reason = Some(reason.into());
        ev
    }

    pub fn stt_hold_started(mode: &str, buffered_ms: u32) -> Self {
        let mut ev = Self::base(SpeechEventKind::SttHoldStarted);
        ev.hold_mode = Some(mode.to_string());
        ev.buffered_ms = Some(buffered_ms);
        ev
    }

    pub fn stt_hold_ended(outcome: &str, buffered_ms: u32, dropped_ms: u32) -> Self {
        let mut ev = Self::base(SpeechEventKind::SttHoldEnded);
        ev.hold_outcome = Some(outcome.to_string());
        ev.buffered_ms = Some(buffered_ms);
        ev.dropped_ms = Some(dropped_ms);
        ev
    }

    pub fn with_utterance_id(mut self, utterance_id: Option<String>) -> Self {
        self.utterance_id = utterance_id;
        self
    }

    pub fn user_speaking_start(utterance_id: impl Into<String>) -> Self {
        Self::base(SpeechEventKind::UserSpeakingStart).with_utterance_id(Some(utterance_id.into()))
    }

    pub fn user_speaking_end(utterance_id: Option<String>) -> Self {
        Self::base(SpeechEventKind::UserSpeakingEnd).with_utterance_id(utterance_id)
    }

    pub fn user_speech_partial(text: impl Into<String>, utterance_id: Option<String>) -> Self {
        Self::base(SpeechEventKind::UserSpeechPartial)
            .with_utterance_id(utterance_id)
            .with_text(text)
    }

    pub fn user_speech_final(text: impl Into<String>, utterance_id: Option<String>) -> Self {
        Self::base(SpeechEventKind::UserSpeechFinal)
            .with_utterance_id(utterance_id)
            .with_text(text)
    }

    pub fn user_speech_final_replay(
        text: impl Into<String>,
        utterance_id: Option<String>,
        replaces_utterance_id: impl Into<String>,
    ) -> Self {
        Self::user_speech_final(text, utterance_id)
            .with_replay(true)
            .with_replaces_utterance_id(Some(replaces_utterance_id.into()))
    }

    /// Final produced by `release_stt_hold({ replay: true })`; `replaces` is set when an original existed.
    pub fn user_speech_final_held_replay(
        text: impl Into<String>,
        utterance_id: Option<String>,
        replaces_utterance_id: Option<String>,
    ) -> Self {
        Self::user_speech_final(text, utterance_id)
            .with_replay(true)
            .with_replaces_utterance_id(replaces_utterance_id)
    }

    pub fn user_language(lang: impl Into<String>, utterance_id: Option<String>) -> Self {
        let code = lang.into();
        Self::base(SpeechEventKind::UserLanguage)
            .with_utterance_id(utterance_id)
            .with_language(Some(code.clone()))
            .with_text(code)
    }

    pub fn language_id_skipped(reason: &str, speech_ms: u32, utterance_id: Option<String>) -> Self {
        let mut ev = Self::base(SpeechEventKind::LanguageIdSkipped).with_utterance_id(utterance_id);
        ev.reason = Some(reason.to_string());
        ev.speech_ms = Some(speech_ms);
        ev
    }

    pub fn agent_speaking_start() -> Self {
        Self::base(SpeechEventKind::AgentSpeakingStart)
    }

    /// `agent_speaking_start` carrying time-to-first-audio timing for the reply that started playback.
    pub fn agent_speaking_start_with_timing(
        first_audio_ms: Option<u32>,
        first_chunk_ms: Option<u32>,
    ) -> Self {
        let mut ev = Self::base(SpeechEventKind::AgentSpeakingStart);
        ev.first_audio_ms = first_audio_ms;
        ev.first_chunk_ms = first_chunk_ms;
        ev
    }

    pub fn agent_speaking_end() -> Self {
        Self::base(SpeechEventKind::AgentSpeakingEnd)
    }

    pub fn vad_triggered() -> Self {
        Self::base(SpeechEventKind::VadTriggered)
    }

    pub fn stt_stream_start() -> Self {
        Self::base(SpeechEventKind::SttStreamStart)
    }

    pub fn stt_stream_end() -> Self {
        Self::base(SpeechEventKind::SttStreamEnd)
    }

    pub fn user_stt_start() -> Self {
        Self::base(SpeechEventKind::UserSttStart)
    }

    pub fn user_stt_end() -> Self {
        Self::base(SpeechEventKind::UserSttEnd)
    }

    pub fn user_stt_not_found() -> Self {
        Self::base(SpeechEventKind::UserSttNotFound)
    }

    pub fn barge_in() -> Self {
        Self::base(SpeechEventKind::BargeIn)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::base(SpeechEventKind::Error).with_error(message)
    }

    pub fn stt_config_updated(config: &SttConfig) -> Self {
        Self::base(SpeechEventKind::SttConfigUpdated)
            .with_language(config.language.clone())
            .with_model_path(config.model_path.clone())
            .with_endpoint(config.endpoint.clone())
    }

    pub fn tts_config_updated(config: &TtsConfig) -> Self {
        Self::base(SpeechEventKind::TtsConfigUpdated)
            .with_voice(config.voice.clone())
            .with_model_path(config.model_path.clone())
            .with_endpoint(config.endpoint.clone())
    }

    fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }

    fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    fn with_replay(mut self, replay: bool) -> Self {
        self.replay = Some(replay);
        self
    }

    fn with_replaces_utterance_id(mut self, id: Option<String>) -> Self {
        self.replaces_utterance_id = id;
        self
    }

    fn with_voice(mut self, voice: Option<String>) -> Self {
        self.voice = voice;
        self
    }

    fn with_model_path(mut self, model_path: Option<String>) -> Self {
        self.model_path = model_path;
        self
    }

    fn with_endpoint(mut self, endpoint: Option<String>) -> Self {
        self.endpoint = endpoint;
        self
    }
}

/// Broadcast bus for speech events (callback + `pull_speech_event` / stream subscribers).
///
/// Capacity 256; lagging receivers may drop events under load.
#[derive(Clone)]
pub struct SpeechEventBus {
    tx: broadcast::Sender<SpeechEvent>,
}

impl SpeechEventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SpeechEvent> {
        self.tx.subscribe()
    }

    pub fn emit(&self, event: SpeechEvent) {
        let _ = self.tx.send(event);
    }
}

impl Default for SpeechEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod user_language_event_tests {
    use super::*;

    #[test]
    fn user_language_sets_text_and_language() {
        let event = SpeechEvent::user_language("de", Some("utt-1".into()));
        assert_eq!(event.kind, SpeechEventKind::UserLanguage);
        assert_eq!(event.language.as_deref(), Some("de"));
        assert_eq!(event.text.as_deref(), Some("de"));
        assert_eq!(event.utterance_id.as_deref(), Some("utt-1"));
    }

    #[test]
    fn language_id_skipped_sets_reason_and_speech_ms() {
        let event = SpeechEvent::language_id_skipped("too_short", 120, Some("utt-1".into()));
        assert_eq!(event.kind, SpeechEventKind::LanguageIdSkipped);
        assert_eq!(event.reason.as_deref(), Some("too_short"));
        assert_eq!(event.speech_ms, Some(120));
        assert_eq!(event.utterance_id.as_deref(), Some("utt-1"));
        assert!(event.text.is_none());
    }

    #[test]
    fn agent_speaking_start_timing_defaults_to_none() {
        let plain = SpeechEvent::agent_speaking_start();
        assert_eq!(plain.kind, SpeechEventKind::AgentSpeakingStart);
        assert_eq!(plain.first_audio_ms, None);
        assert_eq!(plain.first_chunk_ms, None);

        let timed = SpeechEvent::agent_speaking_start_with_timing(Some(240), Some(180));
        assert_eq!(timed.kind, SpeechEventKind::AgentSpeakingStart);
        assert_eq!(timed.first_audio_ms, Some(240));
        assert_eq!(timed.first_chunk_ms, Some(180));
    }
}
