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
//! | `AgentSpeakingStart` | First TTS PCM frame queued to outbound |
//! | `AgentSpeakingEnd` | TTS queue drained — **only on the agent that plays TTS** |
//! | `VadTriggered` | VAD `SpeechStart` when `vad.enabled` — opens STT listen for this utterance |
//! | `SttStreamStart` / `SttStreamEnd` | STT vendor PCM feed opened / closed for an utterance |
//! | `UserSttStart` / `UserSttEnd` | STT recognition session for one user utterance |
//! | `UserSttNotFound` | VAD fired but no STT partial within `sttListenTimeoutMs` |
//! | `BargeIn` | Barge-in path (semantic STT partial and/or VAD during agent TTS) |
//! | `Error` | Vendor or internal failure |

use tokio::sync::broadcast;

/// Speech lifecycle events emitted by the voice agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechEventKind {
    UserSpeakingStart,
    UserSpeakingEnd,
    UserSpeechPartial,
    UserSpeechFinal,
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
}

/// A speech event with optional payload text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechEvent {
    pub kind: SpeechEventKind,
    pub text: Option<String>,
    pub error: Option<String>,
}

impl SpeechEvent {
    pub fn user_speaking_start() -> Self {
        Self {
            kind: SpeechEventKind::UserSpeakingStart,
            text: None,
            error: None,
        }
    }

    pub fn user_speaking_end() -> Self {
        Self {
            kind: SpeechEventKind::UserSpeakingEnd,
            text: None,
            error: None,
        }
    }

    pub fn user_speech_partial(text: impl Into<String>) -> Self {
        Self {
            kind: SpeechEventKind::UserSpeechPartial,
            text: Some(text.into()),
            error: None,
        }
    }

    pub fn user_speech_final(text: impl Into<String>) -> Self {
        Self {
            kind: SpeechEventKind::UserSpeechFinal,
            text: Some(text.into()),
            error: None,
        }
    }

    pub fn agent_speaking_start() -> Self {
        Self {
            kind: SpeechEventKind::AgentSpeakingStart,
            text: None,
            error: None,
        }
    }

    pub fn agent_speaking_end() -> Self {
        Self {
            kind: SpeechEventKind::AgentSpeakingEnd,
            text: None,
            error: None,
        }
    }

    pub fn vad_triggered() -> Self {
        Self {
            kind: SpeechEventKind::VadTriggered,
            text: None,
            error: None,
        }
    }

    pub fn stt_stream_start() -> Self {
        Self {
            kind: SpeechEventKind::SttStreamStart,
            text: None,
            error: None,
        }
    }

    pub fn stt_stream_end() -> Self {
        Self {
            kind: SpeechEventKind::SttStreamEnd,
            text: None,
            error: None,
        }
    }

    pub fn user_stt_start() -> Self {
        Self {
            kind: SpeechEventKind::UserSttStart,
            text: None,
            error: None,
        }
    }

    pub fn user_stt_end() -> Self {
        Self {
            kind: SpeechEventKind::UserSttEnd,
            text: None,
            error: None,
        }
    }

    pub fn user_stt_not_found() -> Self {
        Self {
            kind: SpeechEventKind::UserSttNotFound,
            text: None,
            error: None,
        }
    }

    pub fn barge_in() -> Self {
        Self {
            kind: SpeechEventKind::BargeIn,
            text: None,
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            kind: SpeechEventKind::Error,
            text: None,
            error: Some(message.into()),
        }
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
