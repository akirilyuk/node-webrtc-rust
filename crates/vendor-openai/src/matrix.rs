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

/// Documented Realtime WebSocket base (Getting started + WebSockets guide).
/// Connect with `Authorization: Bearer <ek_…>` from `POST /v1/realtime/client_secrets`.
pub const REALTIME_WEBSOCKET_URL: &str = "wss://api.openai.com/v1/realtime";

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
    fn every_documented_stt_model_has_explicit_matrix_row() {
        let rows: &[(&str, SttDefaultTransport, bool)] = &[
            ("whisper-1", SttDefaultTransport::FileJson, false),
            ("gpt-4o-mini-transcribe", SttDefaultTransport::FileSse, true),
            ("gpt-4o-transcribe", SttDefaultTransport::FileSse, true),
            ("gpt-4o-transcribe-diarize", SttDefaultTransport::FileSse, true),
            ("gpt-transcribe", SttDefaultTransport::FileSse, true),
            ("gpt-live-transcribe", SttDefaultTransport::RealtimeLive, true),
        ];
        assert_eq!(
            DOCUMENTED_STT_MODELS.len(),
            rows.len(),
            "keep DOCUMENTED_STT_MODELS and this table in sync"
        );
        for (model, expected_transport, file_sse) in rows {
            assert_eq!(
                stt_default_transport(model).unwrap(),
                *expected_transport,
                "default transport for `{model}`"
            );
            assert_eq!(
                stt_file_sse_supported(model),
                *file_sse,
                "stt_file_sse_supported for `{model}`"
            );
            assert!(
                DOCUMENTED_STT_MODELS.contains(model),
                "`{model}` missing from DOCUMENTED_STT_MODELS"
            );
        }
        for model in DOCUMENTED_STT_MODELS {
            assert!(
                rows.iter().any(|(m, _, _)| m == model),
                "`{model}` missing from explicit matrix test rows"
            );
        }
    }

    #[test]
    fn gpt_4o_transcribe_and_diarize_file_sse_not_realtime() {
        for model in ["gpt-4o-transcribe", "gpt-4o-transcribe-diarize"] {
            assert_eq!(
                stt_default_transport(model).unwrap(),
                SttDefaultTransport::FileSse
            );
            assert!(stt_file_sse_supported(model));
            assert!(!stt_uses_realtime_ws(SttDefaultTransport::FileSse));
        }
    }

    #[test]
    fn realtime_websocket_base_has_no_query_or_intent() {
        assert_eq!(REALTIME_WEBSOCKET_URL, "wss://api.openai.com/v1/realtime");
        assert!(!REALTIME_WEBSOCKET_URL.contains('?'));
        assert!(!REALTIME_WEBSOCKET_URL.contains("intent="));
    }

    #[test]
    fn unknown_stt_model_errors() {
        assert!(stt_default_transport("not-a-model").is_err());
        assert!(stt_default_transport("whisper-2").is_err());
        assert!(stt_default_transport("my-transcribe").is_err());
    }
}
