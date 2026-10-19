//! Hardcoded Google Cloud STT/TTS capability matrix from published docs only.
//!
//! STT Chirp / V2: <https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3>
//! STT V2 recognizers: <https://cloud.google.com/speech-to-text/v2/docs/recognizers>
//! TTS streaming: <https://docs.cloud.google.com/text-to-speech/docs/create-audio-text-streaming>
//! TTS voices: <https://cloud.google.com/text-to-speech/docs/voices>

use std::sync::LazyLock;
use std::time::Duration;

use node_webrtc_rust_voice_catalog as voice_catalog;

/// TCP connect bound of the pooled REST client (`infra.unbounded-external-wait`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-request bound for the REST calls (V1 `speech:recognize`, `text:synthesize`). Google had
/// no timeout before; 60 s matches the other REST vendors in this workspace.
pub const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an idle pooled connection is kept for the next request.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// TCP keepalive on pooled connections.
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Documented Speech-to-Text model identifiers (V2 table + V1 `latest_long`).
pub static DOCUMENTED_STT_MODELS: LazyLock<&'static [&'static str]> =
    LazyLock::new(|| voice_catalog::stt_models("google").expect("google in voice catalog"));

/// Doc URL when V2 streaming requires project/location/recognizer.
pub const STT_V2_STREAMING_DOC: &str =
    "https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3";
pub const STT_V2_RECOGNIZER_DOC: &str =
    "https://cloud.google.com/speech-to-text/v2/docs/recognizers";

pub const TTS_VOICES_DOC: &str = "https://cloud.google.com/text-to-speech/docs/voices";

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

/// Location used when none is configured. Chirp 3 page: "Chirp 3 is available in the following
/// Google Cloud regions, with more planned: `us (multi-region)` GA, `eu (multi-region)` GA". The
/// recognizers page documents `projects/{PROJECT_ID}/locations/global/recognizers/_` for other
/// models, but `global` is not a Chirp 3 location, so the default is `us`.
pub const DEFAULT_V2_LOCATION: &str = "us";
/// The recognizer id for a request without a stored recognizer. Recognizers page: "Recognizers
/// are optional in recognition requests. To make a request without a recognizer, use the
/// recognizer resource ID `_` in the location you are making a request." The model and language
/// travel in the inline `config` of the first `StreamingRecognizeRequest`.
pub const IMPLICIT_RECOGNIZER: &str = "_";

/// Resolves the V2 recognizer for a streaming model, from `endpoint`
/// (`projects/{p}/locations/{l}/recognizers/{r}`) or the env names below (read through `env`).
///
/// - `GOOGLE_CLOUD_PROJECT` (required without an endpoint)
/// - `GOOGLE_SPEECH_LOCATION` (default [`DEFAULT_V2_LOCATION`])
/// - `GOOGLE_SPEECH_RECOGNIZER` (default [`IMPLICIT_RECOGNIZER`])
///
/// `Ok(None)` for models that do not stream through V2.
pub(crate) fn resolve_v2_locator(
    model: &str,
    endpoint: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<GoogleSttV2Locator>, String> {
    if !stt_uses_v2_streaming(stt_default_transport(model)?) {
        return Ok(None);
    }
    let value = |name: &str| {
        env(name)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    if let Some(endpoint) = endpoint.map(str::trim).filter(|s| !s.is_empty()) {
        return parse_recognizer_resource(endpoint).map(Some).ok_or_else(|| {
            format!(
                "Google STT endpoint must be projects/{{project}}/locations/{{location}}/recognizers/{{recognizer}}; see {STT_V2_RECOGNIZER_DOC}"
            )
        });
    }
    let project = value("GOOGLE_CLOUD_PROJECT").ok_or_else(|| {
        format!(
            "Google STT model `{model}` streams through V2 and needs a project: set \
             GOOGLE_CLOUD_PROJECT (or stt.endpoint = projects/…/locations/…/recognizers/…). \
             See {STT_V2_RECOGNIZER_DOC} and {STT_V2_STREAMING_DOC}"
        )
    })?;
    Ok(Some(GoogleSttV2Locator {
        project,
        location: value("GOOGLE_SPEECH_LOCATION").unwrap_or_else(|| DEFAULT_V2_LOCATION.into()),
        recognizer: value("GOOGLE_SPEECH_RECOGNIZER").unwrap_or_else(|| IMPLICIT_RECOGNIZER.into()),
    }))
}

impl GoogleSttV2Locator {
    /// `projects/{project}/locations/{location}/recognizers/{recognizer}`.
    pub fn resource_name(&self) -> String {
        format!(
            "projects/{}/locations/{}/recognizers/{}",
            self.project, self.location, self.recognizer
        )
    }

    /// gRPC endpoint for the location. Streaming guide: "us-speech.googleapis.com for us"; the
    /// regional endpoints page lists `https://eu-speech.googleapis.com` and
    /// `https://us-speech.googleapis.com`. `global` uses the default endpoint (`None`).
    pub fn endpoint(&self) -> Option<String> {
        if self.location == "global" {
            None
        } else {
            Some(format!("https://{}-speech.googleapis.com", self.location))
        }
    }
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

/// Streaming Cloud TTS is only compatible with Chirp 3 HD voice names (doc quickstart).
pub fn voice_supports_streaming_synthesize(voice: &str) -> bool {
    voice.contains("Chirp3-HD")
}

/// Voice names from the catalog snapshot of the Cloud TTS `voices.list` response.
pub fn validate_tts_voice(voice: &str) -> Result<(), String> {
    let documented = voice_catalog::google_tts_voices();
    if documented.contains(&voice) {
        return Ok(());
    }
    Err(format!(
        "unsupported Google TTS voice `{voice}`; must be a voice name from the Cloud TTS voices list ({} voices); see {TTS_VOICES_DOC}",
        documented.len()
    ))
}

pub fn default_tts_voice() -> &'static str {
    voice_catalog::default_tts_voice("google").expect("google default TTS voice in voice catalog")
}

pub fn default_stt_model() -> &'static str {
    "latest_long"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chirp_uses_v2_streaming() {
        assert_eq!(
            stt_default_transport("chirp_3").unwrap(),
            SttDefaultTransport::V2StreamingRecognize
        );
        assert!(stt_uses_v2_streaming(
            SttDefaultTransport::V2StreamingRecognize
        ));
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
        assert!(voice_supports_streaming_synthesize(
            "en-US-Chirp3-HD-Charon"
        ));
        assert!(!voice_supports_streaming_synthesize("en-US-Neural2-A"));
    }

    #[test]
    fn google_rejects_unknown_voice() {
        assert!(validate_tts_voice("en-US-Nobody-Z").is_err());
    }

    #[test]
    fn google_accepts_chirp3_hd_voice() {
        assert!(validate_tts_voice("en-US-Chirp3-HD-Charon").is_ok());
        assert!(validate_tts_voice(default_tts_voice()).is_ok());
    }

    #[test]
    fn parse_recognizer_endpoint() {
        let loc = parse_recognizer_resource("projects/my-proj/locations/us/recognizers/my-rec")
            .expect("parse");
        assert_eq!(loc.project, "my-proj");
        assert_eq!(loc.location, "us");
        assert_eq!(loc.recognizer, "my-rec");
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn implicit_recognizer_defaults_to_the_chirp_3_location() {
        let loc = resolve_v2_locator("chirp_3", None, env(&[("GOOGLE_CLOUD_PROJECT", "proj")]))
            .unwrap()
            .unwrap();
        assert_eq!(
            loc.resource_name(),
            "projects/proj/locations/us/recognizers/_"
        );
        assert_eq!(
            loc.endpoint().as_deref(),
            Some("https://us-speech.googleapis.com")
        );
    }

    #[test]
    fn env_overrides_location_and_recognizer() {
        let loc = resolve_v2_locator(
            "chirp_3",
            None,
            env(&[
                ("GOOGLE_CLOUD_PROJECT", "proj"),
                ("GOOGLE_SPEECH_LOCATION", "eu"),
                ("GOOGLE_SPEECH_RECOGNIZER", "mine"),
            ]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            loc.resource_name(),
            "projects/proj/locations/eu/recognizers/mine"
        );
        assert_eq!(
            loc.endpoint().as_deref(),
            Some("https://eu-speech.googleapis.com")
        );
    }

    #[test]
    fn endpoint_wins_over_env_and_global_uses_the_default_endpoint() {
        let loc = resolve_v2_locator(
            "chirp_3",
            Some("projects/p/locations/global/recognizers/r"),
            env(&[("GOOGLE_CLOUD_PROJECT", "other")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(loc.project, "p");
        assert_eq!(loc.endpoint(), None);
    }

    #[test]
    fn missing_project_is_a_named_error() {
        let err = resolve_v2_locator("chirp_3", None, env(&[])).unwrap_err();
        assert!(err.contains("GOOGLE_CLOUD_PROJECT"), "{err}");
        assert!(err.contains("chirp_3"), "{err}");
    }

    #[test]
    fn malformed_endpoint_is_an_error_and_v1_models_need_no_locator() {
        assert!(resolve_v2_locator("chirp_3", Some("nonsense"), env(&[])).is_err());
        assert_eq!(
            resolve_v2_locator("latest_long", None, env(&[])).unwrap(),
            None
        );
    }

    #[test]
    fn every_documented_stt_model_has_row() {
        for model in *DOCUMENTED_STT_MODELS {
            assert!(
                stt_default_transport(model).is_ok(),
                "missing row for `{model}`"
            );
        }
    }
}

/// Validate `TtsConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_tts_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "google",
        "tts",
        requested,
        voice_catalog::tts_transports("google"),
        voice_catalog::default_tts_transport("google"),
    )
    .map(|_| ())
}

/// Validate `SttConfig.transport` against the voice catalog (`None` / `auto` = the
/// catalog default). An unsupported value is a config error, raised at provider construction.
pub(crate) fn check_stt_transport(
    requested: Option<node_webrtc_rust_speech::config::SpeechTransport>,
) -> node_webrtc_rust_speech::error::SpeechResult<()> {
    node_webrtc_rust_speech::resolve_transport(
        "google",
        "stt",
        requested,
        voice_catalog::stt_transports("google"),
        voice_catalog::default_stt_transport("google"),
    )
    .map(|_| ())
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use node_webrtc_rust_speech::config::SpeechTransport;
    use node_webrtc_rust_speech::error::SpeechError;

    #[test]
    fn tts_transport_is_decided_by_the_model() {
        assert!(check_tts_transport(None).is_ok());
        assert!(check_tts_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(voice_catalog::tts_transports("google").is_empty());
        for requested in [SpeechTransport::Http, SpeechTransport::Websocket] {
            match check_tts_transport(Some(requested)) {
                Err(SpeechError::Config(message)) => assert_eq!(
                    message,
                    "google tts transport is decided by the model; use auto"
                ),
                other => panic!("expected a Config error, got {other:?}"),
            }
        }
    }

    #[test]
    fn stt_transport_is_decided_by_the_model() {
        assert!(check_stt_transport(None).is_ok());
        assert!(check_stt_transport(Some(SpeechTransport::Auto)).is_ok());
        assert!(voice_catalog::stt_transports("google").is_empty());
        for requested in [SpeechTransport::Http, SpeechTransport::Websocket] {
            match check_stt_transport(Some(requested)) {
                Err(SpeechError::Config(message)) => assert_eq!(
                    message,
                    "google stt transport is decided by the model; use auto"
                ),
                other => panic!("expected a Config error, got {other:?}"),
            }
        }
    }
}
