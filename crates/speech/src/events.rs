//! Speech event bus and event types.

use tokio::sync::broadcast;

/// Speech lifecycle events emitted by the voice agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechEventKind {
    UserSpeakingStart,
    UserSpeakingEnd,
    UserSpeechPartial,
    UserSpeechFinal,
    AgentSpeakingStart,
    AgentSpeakingEnd,
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

/// Broadcast bus for speech events (supports callback + stream subscribers).
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
