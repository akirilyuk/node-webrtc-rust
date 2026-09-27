//! Hardcoded OpenAI STT/TTS capability matrix from published docs (no `GET /v1/models`).
//!
//! STT: <https://developers.openai.com/api/docs/guides/speech-to-text>,
//! <https://developers.openai.com/api/docs/guides/realtime-transcription>,
//! <https://developers.openai.com/api/docs/guides/realtime-websocket>
//!
//! TTS: <https://developers.openai.com/api/docs/guides/text-to-speech>,
//! <https://developers.openai.com/api/docs/api-reference/audio/createSpeech>

/// Documented transcription models (VoiceAgent auto-pick and error messages).
pub const DOCUMENTED_STT_MODELS: &[&str] = &[
    "whisper-1",
    "gpt-4o-mini-transcribe",
    "gpt-4o-transcribe",
    "gpt-4o-transcribe-diarize",
    "gpt-transcribe",
    "gpt-live-transcribe",
];

/// How an STT model is reached by default for bounded VoiceAgent utterances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttDefaultTransport {
    /// `POST /v1/audio/transcriptions` JSON `{text}` (`stream` omitted/false).
    FileJson,
    /// Same POST with `stream=true`; SSE `transcript.text.delta` / `transcript.text.done`.
    FileSse,
    /// Realtime WS `session.type=transcription`, append while speaking (`gpt-live-transcribe`).
    RealtimeLive,
}

/// Explicit Realtime committed-turn path for tests (`gpt-transcribe` after `input_audio_buffer.commit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttRealtimeMode {
    Live,
    CommittedTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsStreamFormat {
    /// Chunked raw PCM (`stream_format=audio`).
    Audio,
    /// `stream_format=sse` — not for `tts-1` / `tts-1-hd`.
    Sse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtsDeliveryPlan {
    pub try_stream: TtsStreamFormat,
    pub allow_full_body_fallback: bool,
}

/// Default VoiceAgent transport for a configured STT model string.
pub fn stt_default_transport(model: &str) -> Result<SttDefaultTransport, String> {
    match model {
        "whisper-1" => Ok(SttDefaultTransport::FileJson),
        "gpt-4o-mini-transcribe" | "gpt-4o-transcribe" | "gpt-4o-transcribe-diarize" => {
            Ok(SttDefaultTransport::FileSse)
        }
        "gpt-transcribe" => Ok(SttDefaultTransport::FileSse),
        "gpt-live-transcribe" => Ok(SttDefaultTransport::RealtimeLive),
        other if other.starts_with("whisper") => Ok(SttDefaultTransport::FileJson),
        other if other.contains("transcribe") => Ok(SttDefaultTransport::FileSse),
        unknown => Err(format!(
            "unsupported OpenAI STT model `{unknown}`; documented models: {}",
            DOCUMENTED_STT_MODELS.join(", ")
        )),
    }
}

pub fn stt_file_sse_supported(model: &str) -> bool {
    !matches!(model, "whisper-1")
}

pub fn stt_uses_realtime_ws(default: SttDefaultTransport) -> bool {
    matches!(default, SttDefaultTransport::RealtimeLive)
}

pub fn documented_realtime_model(mode: SttRealtimeMode) -> &'static str {
    match mode {
        SttRealtimeMode::Live => "gpt-live-transcribe",
        SttRealtimeMode::CommittedTurn => "gpt-transcribe",
    }
}

/// Realtime **conversation** sessions use `?model=` (e.g. `gpt-realtime-2.1`) per
/// <https://developers.openai.com/api/docs/guides/realtime-websocket>. Transcription-only
/// sessions (`session.type=transcription`) require a transcription WebSocket endpoint;
/// the API rejects transcription `session.update` on a conversation `?model=` connection.
pub const REALTIME_CONVERSATION_SESSION_MODEL: &str = "gpt-realtime-2.1";

/// WebSocket URL for Realtime **transcription** sessions (`gpt-live-transcribe`, committed-turn `gpt-transcribe`).
/// Transcription models are passed only in `session.update` → `audio.input.transcription.model`.
pub fn realtime_transcription_websocket_url() -> &'static str {
    "wss://api.openai.com/v1/realtime?intent=transcription"
}

pub fn tts_delivery_plan(model: &str) -> TtsDeliveryPlan {
    match model {
        "tts-1" | "tts-1-hd" => TtsDeliveryPlan {
            try_stream: TtsStreamFormat::Audio,
            allow_full_body_fallback: true,
        },
        "gpt-4o-mini-tts" | "gpt-4o-mini-tts-2025-12-15" => TtsDeliveryPlan {
            try_stream: TtsStreamFormat::Audio,
            allow_full_body_fallback: true,
        },
        _ => TtsDeliveryPlan {
            try_stream: TtsStreamFormat::Audio,
            allow_full_body_fallback: true,
        },
    }
}

pub fn tts_sse_allowed(model: &str) -> bool {
    !matches!(model, "tts-1" | "tts-1-hd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_file_json_no_sse() {
        assert_eq!(
            stt_default_transport("whisper-1").unwrap(),
            SttDefaultTransport::FileJson
        );
        assert!(!stt_file_sse_supported("whisper-1"));
    }

    #[test]
    fn mini_transcribe_file_sse_not_realtime() {
        assert_eq!(
            stt_default_transport("gpt-4o-mini-transcribe").unwrap(),
            SttDefaultTransport::FileSse
        );
        assert!(!stt_uses_realtime_ws(SttDefaultTransport::FileSse));
    }

    #[test]
    fn gpt_transcribe_default_file_sse() {
        assert_eq!(
            stt_default_transport("gpt-transcribe").unwrap(),
            SttDefaultTransport::FileSse
        );
    }

    #[test]
    fn live_transcribe_realtime_only() {
        assert_eq!(
            stt_default_transport("gpt-live-transcribe").unwrap(),
            SttDefaultTransport::RealtimeLive
        );
        assert!(stt_uses_realtime_ws(SttDefaultTransport::RealtimeLive));
    }

    #[test]
    fn tts1_no_sse() {
        assert!(!tts_sse_allowed("tts-1"));
        assert!(!tts_sse_allowed("tts-1-hd"));
    }

    #[test]
    fn gpt_mini_tts_allows_sse_in_matrix() {
        assert!(tts_sse_allowed("gpt-4o-mini-tts"));
    }

    #[test]
    fn realtime_transcription_url_uses_intent_not_transcription_model_in_query() {
        let url = realtime_transcription_websocket_url();
        assert!(url.contains("intent=transcription"));
        assert!(!url.contains("gpt-live-transcribe"));
        assert!(!url.contains("model=gpt-transcribe"));
    }

    #[test]
    fn unknown_stt_model_errors() {
        assert!(stt_default_transport("not-a-model").is_err());
    }
}
