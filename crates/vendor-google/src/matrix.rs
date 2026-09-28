//! Hardcoded Google Cloud STT/TTS capability matrix from published docs only.
//!
//! STT Chirp / V2: <https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3>
//! STT V2 recognizers: <https://cloud.google.com/speech-to-text/v2/docs/recognizers>
//! TTS streaming: <https://docs.cloud.google.com/text-to-speech/docs/create-audio-text-streaming>
//! TTS voices: <https://cloud.google.com/text-to-speech/docs/voices>

use node_webrtc_rust_speech::config::SttConfig;

/// Documented Speech-to-Text model identifiers (V2 table + V1 `latest_long`).
pub const DOCUMENTED_STT_MODELS: &[&str] = &["chirp_3", "chirp_2", "telephony", "latest_long"];

/// Doc URL when V2 streaming requires project/location/recognizer.
pub const STT_V2_STREAMING_DOC: &str =
    "https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3";
pub const STT_V2_RECOGNIZER_DOC: &str =
    "https://cloud.google.com/speech-to-text/v2/docs/recognizers";

pub const TTS_STREAMING_DOC: &str =
    "https://docs.cloud.google.com/text-to-speech/docs/create-audio-text-streaming";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttDefaultTransport {
    /// V1 `POST …/v1/speech:recognize` (batch / `latest_long`).
    V1RestRecognize,
    /// V2 `Speech.StreamingRecognize` with `interim_results` (Chirp / telephony).
    V2StreamingRecognize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleSttV2Locator {
    pub project: String,
    pub location: String,
    pub recognizer: String,
}

pub fn stt_default_transport(model: &str) -> Result<SttDefaultTransport, String> {
    match model {
        "chirp_3" | "chirp_2" | "telephony" => Ok(SttDefaultTransport::V2StreamingRecognize),
        "latest_long" => Ok(SttDefaultTransport::V1RestRecognize),
        unknown => Err(format!(
            "unsupported Google STT model `{unknown}`; documented models: {}",
            DOCUMENTED_STT_MODELS.join(", ")
        )),
    }
}

pub fn stt_uses_v2_streaming(transport: SttDefaultTransport) -> bool {
    matches!(transport, SttDefaultTransport::V2StreamingRecognize)
}

/// Resolve V2 recognizer path from `SttConfig.endpoint` (`projects/…/locations/…/recognizers/…`)
/// or `GOOGLE_CLOUD_PROJECT`, `GOOGLE_SPEECH_LOCATION`, `GOOGLE_SPEECH_RECOGNIZER`.
pub fn parse_v2_locator(config: &SttConfig) -> Option<GoogleSttV2Locator> {
    if let Some(endpoint) = config.endpoint.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(loc) = parse_recognizer_resource(endpoint) {
            return Some(loc);
        }
    }
    let project = std::env::var("GOOGLE_CLOUD_PROJECT")
        .ok()
        .filter(|s| !s.is_empty())?;
    let location = std::env::var("GOOGLE_SPEECH_LOCATION")
        .ok()
        .filter(|s| !s.is_empty())?;
    let recognizer = std::env::var("GOOGLE_SPEECH_RECOGNIZER")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some(GoogleSttV2Locator {
        project,
        location,
        recognizer,
    })
}

fn parse_recognizer_resource(path: &str) -> Option<GoogleSttV2Locator> {
    // projects/{project}/locations/{location}/recognizers/{recognizer}
    let parts: Vec<&str> = path.trim().trim_start_matches('/').split('/').collect();
    if parts.len() >= 6
        && parts[0] == "projects"
        && parts[2] == "locations"
        && parts[4] == "recognizers"
    {
        return Some(GoogleSttV2Locator {
            project: parts[1].to_string(),
            location: parts[3].to_string(),
            recognizer: parts[5].to_string(),
        });
    }
    None
}

pub fn require_v2_locator_for_streaming(
    model: &str,
    locator: &Option<GoogleSttV2Locator>,
) -> Result<(), String> {
    if !stt_uses_v2_streaming(stt_default_transport(model)?) {
        return Ok(());
    }
    if locator.is_some() {
        return Ok(());
    }
    Err(format!(
        "Google STT model `{model}` requires V2 StreamingRecognize with a recognizer \
         (set stt.endpoint to projects/…/locations/…/recognizers/… or \
         GOOGLE_CLOUD_PROJECT, GOOGLE_SPEECH_LOCATION, GOOGLE_SPEECH_RECOGNIZER). \
         See {STT_V2_RECOGNIZER_DOC} and {STT_V2_STREAMING_DOC}"
    ))
}

/// Streaming Cloud TTS is only compatible with Chirp 3 HD voice names (doc quickstart).
pub fn voice_supports_streaming_synthesize(voice: &str) -> bool {
    voice.contains("Chirp3-HD")
}

pub fn default_stt_model() -> &'static str {
    "latest_long"
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn chirp_uses_v2_streaming() {
        assert_eq!(
            stt_default_transport("chirp_3").unwrap(),
            SttDefaultTransport::V2StreamingRecognize
        );
        assert!(stt_uses_v2_streaming(SttDefaultTransport::V2StreamingRecognize));
    }

    #[test]
    fn latest_long_v1_rest_only() {
        assert_eq!(
            stt_default_transport("latest_long").unwrap(),
            SttDefaultTransport::V1RestRecognize
        );
    }

    #[test]
    fn chirp3_hd_voice_streams() {
        assert!(voice_supports_streaming_synthesize("en-US-Chirp3-HD-Charon"));
        assert!(!voice_supports_streaming_synthesize("en-US-Neural2-A"));
    }

    #[test]
    fn parse_recognizer_endpoint() {
        let loc = parse_recognizer_resource(
            "projects/my-proj/locations/us/recognizers/my-rec",
        )
        .expect("parse");
        assert_eq!(loc.project, "my-proj");
        assert_eq!(loc.location, "us");
        assert_eq!(loc.recognizer, "my-rec");
    }

    #[test]
    fn streaming_model_requires_locator() {
        assert!(require_v2_locator_for_streaming("chirp_3", &None).is_err());
        let loc = GoogleSttV2Locator {
            project: "p".into(),
            location: "us".into(),
            recognizer: "r".into(),
        };
        assert!(require_v2_locator_for_streaming("chirp_3", &Some(loc)).is_ok());
    }

    #[test]
    fn every_documented_stt_model_has_row() {
        for model in DOCUMENTED_STT_MODELS {
            assert!(stt_default_transport(model).is_ok(), "missing row for `{model}`");
        }
    }

    #[test]
    fn parse_v2_from_stt_config_endpoint() {
        let config = SttConfig {
            provider: SttVendor::Google,
            model: Some("chirp_3".into()),
            model_path: None,
            language: Some("en-US".into()),
            api_key: None,
            endpoint: Some("projects/p/locations/eu/recognizers/_".into()),
        };
        let loc = parse_v2_locator(&config).expect("locator");
        assert_eq!(loc.location, "eu");
    }
}
