//! Hardcoded Deepgram TTS capability matrix from published docs.
//!
//! Aura REST/WS: `/v1/speak` — [Speak request](https://developers.deepgram.com/reference/text-to-speech/speak)
//! Flux REST/WS: `/v2/speak` — server events `Connected`, `SpeechStarted`, `SpeechMetadata`, …
//! ([Flux overview](https://developers.deepgram.com/docs/flux-tts/overview))

use std::time::Duration;

use node_webrtc_rust_speech::config::SpeechTransport;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::resolve_transport;
use node_webrtc_rust_voice_catalog as voice_catalog;

pub const V1_SPEAK_DOC: &str = "https://developers.deepgram.com/reference/text-to-speech/speak";
pub const V2_SPEAK_DOC: &str = "https://developers.deepgram.com/docs/flux-tts/overview";
#[allow(dead_code)] // documentation reference, kept next to the other doc URLs
pub const V2_SERVER_MESSAGES_DOC: &str =
    "https://developers.deepgram.com/docs/flux-tts/server-messages";

/// Default REST base URL (`TtsConfig.endpoint` unset). The WebSocket host is derived from it.
pub const DEFAULT_BASE_URL: &str = "https://api.deepgram.com";

/// TCP + TLS + WebSocket handshake (also bounds each WS control send and the HTTP connect).
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the first audio frame.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait between audio frames once audio has started.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole REST request, headers through the last body byte.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled REST connection is kept for the next request.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// TCP keepalive on pooled REST connections.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

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
    #[allow(dead_code)] // documented fallback, selected only without `live`
    RestFullBody,
}

/// TTS transports this crate implements. Lockstep with `voice_catalog::tts_transports("deepgram")`.
pub fn implemented_tts_transports() -> &'static [&'static str] {
    &["websocket", "http"]
}

/// Resolve `TtsConfig.transport` against the voice catalog (`None` / `auto` = catalog default).
pub fn resolve_tts_transport(requested: Option<SpeechTransport>) -> SpeechResult<SpeechTransport> {
    resolve_transport(
        "deepgram",
        "tts",
        requested,
        voice_catalog::tts_transports("deepgram"),
        voice_catalog::default_tts_transport("deepgram"),
    )?
    .ok_or_else(|| SpeechError::Config("deepgram tts has no catalog default transport".into()))
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

/// WS base for a REST base URL: `https://` → `wss://`, `http://` → `ws://`.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
fn websocket_base(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

pub fn v1_speak_rest_url(base_url: &str, model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V1Aura {
        return Err(format!(
            "model `{model}` is not served on /v1/speak (Aura only)"
        ));
    }
    Ok(format!(
        "{}/v1/speak?model={model}&encoding=linear16&sample_rate=48000",
        base_url.trim_end_matches('/')
    ))
}

pub fn v2_speak_rest_url(base_url: &str, model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V2Flux {
        return Err(format!(
            "model `{model}` is not served on /v2/speak (Flux only)"
        ));
    }
    Ok(format!(
        "{}/v2/speak?model={model}&encoding=linear16&sample_rate=48000",
        base_url.trim_end_matches('/')
    ))
}

pub fn v1_speak_websocket_url(base_url: &str, model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V1Aura {
        return Err(format!(
            "Aura model required for /v1/speak websocket, got `{model}`"
        ));
    }
    Ok(format!(
        "{}/v1/speak?model={model}&encoding=linear16&sample_rate=48000",
        websocket_base(base_url)
    ))
}

pub fn v2_speak_websocket_url(base_url: &str, model: &str) -> Result<String, String> {
    validate_speak_model(model)?;
    if speak_api_version(model)? != SpeakApiVersion::V2Flux {
        return Err(format!(
            "Flux model required for /v2/speak websocket, got `{model}`"
        ));
    }
    Ok(format!(
        "{}/v2/speak?model={model}&encoding=linear16&sample_rate=48000",
        websocket_base(base_url)
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
        assert!(v2_speak_rest_url(DEFAULT_BASE_URL, "aura-2-thalia-en").is_err());
        assert!(v1_speak_rest_url(DEFAULT_BASE_URL, "flux-haley-en").is_err());
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

    #[test]
    fn implemented_transports_match_the_catalog() {
        let mut implemented = implemented_tts_transports().to_vec();
        let mut catalog = voice_catalog::tts_transports("deepgram").to_vec();
        implemented.sort_unstable();
        catalog.sort_unstable();
        assert_eq!(implemented, catalog);
    }

    #[test]
    fn auto_and_none_resolve_to_the_catalog_default() {
        assert_eq!(
            resolve_tts_transport(None).unwrap(),
            SpeechTransport::Websocket
        );
        assert_eq!(
            resolve_tts_transport(Some(SpeechTransport::Auto)).unwrap(),
            SpeechTransport::Websocket
        );
        assert_eq!(
            resolve_tts_transport(Some(SpeechTransport::Http)).unwrap(),
            SpeechTransport::Http
        );
    }

    #[test]
    fn default_urls_unchanged() {
        assert_eq!(
            v1_speak_rest_url(DEFAULT_BASE_URL, "aura-asteria-en").unwrap(),
            "https://api.deepgram.com/v1/speak?model=aura-asteria-en&encoding=linear16&sample_rate=48000"
        );
        assert_eq!(
            v1_speak_websocket_url(DEFAULT_BASE_URL, "aura-asteria-en").unwrap(),
            "wss://api.deepgram.com/v1/speak?model=aura-asteria-en&encoding=linear16&sample_rate=48000"
        );
        assert_eq!(
            v2_speak_rest_url(DEFAULT_BASE_URL, "flux-haley-en").unwrap(),
            "https://api.deepgram.com/v2/speak?model=flux-haley-en&encoding=linear16&sample_rate=48000"
        );
        assert_eq!(
            v2_speak_websocket_url(DEFAULT_BASE_URL, "flux-haley-en").unwrap(),
            "wss://api.deepgram.com/v2/speak?model=flux-haley-en&encoding=linear16&sample_rate=48000"
        );
    }

    #[test]
    fn http_endpoint_maps_to_ws() {
        assert_eq!(
            v1_speak_websocket_url("http://127.0.0.1:9", "aura-asteria-en").unwrap(),
            "ws://127.0.0.1:9/v1/speak?model=aura-asteria-en&encoding=linear16&sample_rate=48000"
        );
        assert_eq!(
            v1_speak_rest_url("http://127.0.0.1:9/", "aura-asteria-en").unwrap(),
            "http://127.0.0.1:9/v1/speak?model=aura-asteria-en&encoding=linear16&sample_rate=48000"
        );
    }
}
