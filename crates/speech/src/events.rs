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
//! | `AgentSpeakingStart` | First TTS PCM frame queued to outbound |
//! | `AgentSpeakingEnd` | TTS queue drained — **only on the agent that plays TTS** |
//! | `VadTriggered` | VAD `SpeechStart` when `vad.enabled` — opens STT listen for this utterance |
//! | `SttStreamStart` / `SttStreamEnd` | STT vendor PCM feed opened / closed for an utterance |
//! | `UserSttStart` / `UserSttEnd` | STT recognition session for one user utterance |
//! | `UserSttNotFound` | VAD fired but no STT partial within `sttListenTimeoutMs` |
//! | `BargeIn` | Barge-in path (semantic STT partial and/or VAD during agent TTS) |
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
        }
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

    pub fn user_language(lang: impl Into<String>, utterance_id: Option<String>) -> Self {
        let code = lang.into();
        Self::base(SpeechEventKind::UserLanguage)
            .with_utterance_id(utterance_id)
            .with_language(Some(code.clone()))
            .with_text(code)
    }

    pub fn agent_speaking_start() -> Self {
        Self::base(SpeechEventKind::AgentSpeakingStart)
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
}
