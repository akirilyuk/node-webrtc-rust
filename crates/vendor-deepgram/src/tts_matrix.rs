//! Hardcoded Deepgram TTS capability matrix from published docs.
//!
//! Aura REST/WS: `/v1/speak` — [Speak request](https://developers.deepgram.com/reference/text-to-speech/speak)
//! Flux REST/WS: `/v2/speak` — server events `Connected`, `SpeechStarted`, `SpeechMetadata`, …
//! ([Flux overview](https://developers.deepgram.com/docs/flux-tts/overview))

pub const V1_SPEAK_DOC: &str = "https://developers.deepgram.com/reference/text-to-speech/speak";
pub const V2_SPEAK_DOC: &str = "https://developers.deepgram.com/docs/flux-tts/overview";
pub const V2_SERVER_MESSAGES_DOC: &str =
    "https://developers.deepgram.com/docs/flux-tts/server-messages";

/// Default `model` query on `/v1/speak` (API reference).
pub const DEFAULT_AURA_MODEL: &str = "aura-asteria-en";

/// Documented Flux voice in quickstart (`flux-haley-en`).
pub const DEFAULT_FLUX_MODEL: &str = "flux-haley-en";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakApiVersion {
    V1Aura,
    V2Flux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsProgressiveTransport {
    /// `wss://api.deepgram.com/v1/speak` — Aura voices only.
    WebSocketV1,
    /// `wss://api.deepgram.com/v2/speak` — Flux `flux-*` models; events include `SpeechStarted`, `SpeechMetadata`.
    WebSocketV2,
    /// Full-body REST when progressive socket is unavailable without `live`.
    RestFullBody,
}

pub fn default_speak_model() -> &'static str {
    DEFAULT_AURA_MODEL
}

pub fn speak_api_version(model: &str) -> Result<SpeakApiVersion, String> {
    if model.starts_with("flux-") {
        return Ok(SpeakApiVersion::V2Flux);
    }
    if model.starts_with("aura") {
        return Ok(SpeakApiVersion::V1Aura);
    }
    Err(format!(
        "unsupported Deepgram speak model `{model}`; use Aura (`aura-*`) on /v1/speak or Flux (`flux-*`) on /v2/speak"
    ))
}

pub fn validate_speak_model(model: &str) -> Result<SpeakApiVersion, String> {
    speak_api_version(model)
}

pub fn tts_progressive_transport(model: &str) -> Result<TtsProgressiveTransport, String> {
    match speak_api_version(model)? {
        SpeakApiVersion::V1Aura => Ok(TtsProgressiveTransport::WebSocketV1),
        SpeakApiVersion::V2Flux => Ok(TtsProgressiveTransport::WebSocketV2),
    }
}

pub fn v1_speak_rest_url(model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V1Aura {
        return Err(format!("model `{model}` is not served on /v1/speak (Aura only)"));
    }
    Ok(format!(
        "https://api.deepgram.com/v1/speak?model={model}&encoding=linear16&sample_rate=48000"
    ))
}

pub fn v2_speak_rest_url(model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V2Flux {
        return Err(format!("model `{model}` is not served on /v2/speak (Flux only)"));
    }
    Ok(format!(
        "https://api.deepgram.com/v2/speak?model={model}&encoding=linear16&sample_rate=48000"
    ))
}

pub fn v1_speak_websocket_url(model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V1Aura {
        return Err(format!("Aura model required for /v1/speak websocket, got `{model}`"));
    }
    Ok(format!(
        "wss://api.deepgram.com/v1/speak?model={model}&encoding=linear16&sample_rate=48000"
    ))
}

pub fn v2_speak_websocket_url(model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V2Flux {
        return Err(format!("Flux model required for /v2/speak websocket, got `{model}`"));
    }
    Ok(format!(
        "wss://api.deepgram.com/v2/speak?model={model}&encoding=linear16&sample_rate=48000"
    ))
}

/// Flux `/v2/speak` JSON control messages (also used on `/v1/speak` for `Speak` / `Flush`).
pub fn speak_json(text: &str) -> String {
    serde_json::json!({ "type": "Speak", "text": text }).to_string()
}

pub fn flush_json() -> &'static str {
    r#"{"type":"Flush"}"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aura_on_v1_flux_on_v2() {
        assert_eq!(
            speak_api_version("aura-2-thalia-en").unwrap(),
            SpeakApiVersion::V1Aura
        );
        assert_eq!(
            speak_api_version("flux-haley-en").unwrap(),
            SpeakApiVersion::V2Flux
        );
    }

    #[test]
    fn cross_endpoint_models_rejected() {
        assert!(v2_speak_rest_url("aura-2-thalia-en").is_err());
        assert!(v1_speak_rest_url("flux-haley-en").is_err());
    }

    #[test]
    fn progressive_transport_matches_model_family() {
        assert_eq!(
            tts_progressive_transport("aura-asteria-en").unwrap(),
            TtsProgressiveTransport::WebSocketV1
        );
        assert_eq!(
            tts_progressive_transport("flux-haley-en").unwrap(),
            TtsProgressiveTransport::WebSocketV2
        );
    }
}
