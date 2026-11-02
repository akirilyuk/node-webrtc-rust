//! One-shot TTS: synthesize a text with a vendor and return the PCM.
//!
//! Uses the same vendor registry and `TryFrom<JsTtsConfig>` conversion as `VoiceAgent`, so a
//! caller (a preview worker) runs the same vendor code as a live session.

use std::time::Duration;

use napi::bindgen_prelude::Buffer;
use napi::{Error, Status};
use napi_derive::napi;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::TtsProvider;

use super::registry::default_vendor_registry;
use super::types::JsTtsConfig;

/// Longest text accepted, in Unicode scalars.
pub const SYNTHESIZE_MAX_CHARS: usize = 1000;
/// Default bound on the whole vendor call.
pub const SYNTHESIZE_DEFAULT_TIMEOUT_MS: u32 = 15_000;
/// Output format: mono s16le at this rate.
const OUT_SAMPLE_RATE: u32 = 24_000;

/// Error `code` seen by JS callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthesizeCode {
    InvalidInput,
    VendorRateLimited,
    VendorKeyInvalid,
    VendorTimeout,
    VendorError,
}

impl AsRef<str> for SynthesizeCode {
    fn as_ref(&self) -> &str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::VendorRateLimited => "vendor_rate_limited",
            Self::VendorKeyInvalid => "vendor_key_invalid",
            Self::VendorTimeout => "vendor_timeout",
            Self::VendorError => "vendor_error",
        }
    }
}

/// Mono 24 kHz s16le PCM of one synthesis.
#[napi(object)]
pub struct JsTtsSynthesis {
    pub pcm: Buffer,
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Debug)]
pub struct SynthesizeError {
    pub code: SynthesizeCode,
    pub message: String,
}

/// napi async functions can only reject with a `Status`, so the code leads the message
/// (`vendor_timeout: …`); the SDK wrapper `synthesizeTts` lifts it into `error.code`.
impl From<SynthesizeError> for Error {
    fn from(value: SynthesizeError) -> Self {
        Error::new(
            Status::GenericFailure,
            format!("{}: {}", value.code.as_ref(), value.message),
        )
    }
}

fn err(code: SynthesizeCode, message: impl Into<String>) -> SynthesizeError {
    SynthesizeError {
        code,
        message: message.into(),
    }
}

/// Removes the api key and bearer tokens from a vendor message.
pub(crate) fn redact(message: &str, api_key: Option<&str>) -> String {
    let mut out = message.to_string();
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        out = out.replace(key, "[redacted]");
    }
    let mut result = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(pos) = rest.to_ascii_lowercase().find("bearer ") {
        let (head, tail) = rest.split_at(pos + "bearer ".len());
        result.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        result.push_str("[redacted]");
        rest = &tail[end..];
    }
    result.push_str(rest);
    result
}

/// HTTP status at the start of a vendor error text: `500 Internal Server Error: …`,
/// `429: …` (numeric status first), or after `HTTP ` / `http ` (`speech HTTP 429 …`,
/// `fishaudio http 429: …`, `HTTP error: 429 …`). Digits elsewhere in the text are ignored.
fn status_of(message: &str) -> Option<u16> {
    let after_http = ["HTTP ", "http "]
        .iter()
        .find_map(|marker| message.find(marker).map(|i| &message[i + marker.len()..]))
        .map(|s| s.trim_start_matches("error: "));
    [after_http, Some(message)]
        .into_iter()
        .flatten()
        .find_map(|s| {
            let digits = s
                .get(..3)
                .filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?;
            matches!(s.as_bytes().get(3), Some(b' ' | b':')).then_some(())?;
            digits.parse().ok()
        })
}

/// Class of a vendor failure from the HTTP status in the vendor's own error text.
fn classify_vendor_message(message: &str) -> SynthesizeCode {
    match status_of(message) {
        Some(429) => SynthesizeCode::VendorRateLimited,
        Some(401 | 403) => SynthesizeCode::VendorKeyInvalid,
        _ => SynthesizeCode::VendorError,
    }
}

/// Stereo 48 kHz s16le to mono 24 kHz s16le (average of each 2-frame, 2-channel group).
fn stereo_48k_to_mono_24k(pcm: &[u8]) -> Vec<u8> {
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let mut out = Vec::with_capacity(samples.len() / 2);
    for group in samples.chunks_exact(4) {
        let sum: i32 = group.iter().map(|s| i32::from(*s)).sum();
        out.extend_from_slice(&((sum / 4) as i16).to_le_bytes());
    }
    out
}

/// Runs one synthesis on `provider` within `timeout`.
pub(crate) async fn run_synthesis(
    provider: &dyn TtsProvider,
    text: &str,
    timeout: Duration,
    api_key: Option<&str>,
) -> Result<Vec<u8>, SynthesizeError> {
    if text.trim().is_empty() {
        return Err(err(SynthesizeCode::InvalidInput, "text is empty"));
    }
    if text.chars().count() > SYNTHESIZE_MAX_CHARS {
        return Err(err(
            SynthesizeCode::InvalidInput,
            format!("text is longer than {SYNTHESIZE_MAX_CHARS} characters"),
        ));
    }
    let chunks =
        match tokio::time::timeout(timeout, provider.synthesize_progressive(text, None)).await {
            Err(_) => {
                return Err(err(
                    SynthesizeCode::VendorTimeout,
                    format!("synthesis did not finish within {} ms", timeout.as_millis()),
                ))
            }
            Ok(Err(error)) => {
                let code = match &error {
                    SpeechError::Vendor { message, .. } => classify_vendor_message(message),
                    _ => SynthesizeCode::VendorError,
                };
                return Err(err(code, redact(&error.to_string(), api_key)));
            }
            Ok(Ok(chunks)) => chunks,
        };
    let mut stereo = Vec::new();
    for chunk in &chunks {
        stereo.extend_from_slice(&chunk.pcm);
    }
    let pcm = stereo_48k_to_mono_24k(&stereo);
    if pcm.is_empty() {
        return Err(err(SynthesizeCode::VendorError, "vendor returned no audio"));
    }
    Ok(pcm)
}

/// Synthesizes `text` once and returns mono 24 kHz s16le PCM.
///
/// Rejects with an error whose message starts with `<code>: `, code one of `invalid_input`,
/// `vendor_rate_limited`, `vendor_key_invalid`, `vendor_timeout`, `vendor_error`. The two status
/// codes are set when the vendor error text carries the HTTP status (429; 401 or 403): OpenAI,
/// Azure, Groq, Deepgram, ElevenLabs, Cartesia, Fish Audio and Mistral do. Any other failure is
/// `vendor_error`. Messages never contain the api key. `timeoutMs` bounds the whole call
/// (default 15000).
#[napi]
pub async fn synthesize_tts(
    config: JsTtsConfig,
    text: String,
    timeout_ms: Option<u32>,
) -> napi::Result<JsTtsSynthesis> {
    let api_key = config.api_key.clone();
    let invalid = |message: String| SynthesizeError {
        code: SynthesizeCode::InvalidInput,
        message: redact(&message, api_key.as_deref()),
    };
    let tts_config = TtsConfig::try_from(config).map_err(|e| invalid(e.reason))?;
    let provider = default_vendor_registry()
        .create_tts(&tts_config)
        .map_err(|e| invalid(e.to_string()))?;
    let timeout = Duration::from_millis(u64::from(
        timeout_ms.unwrap_or(SYNTHESIZE_DEFAULT_TIMEOUT_MS),
    ));
    let pcm = run_synthesis(provider.as_ref(), &text, timeout, api_key.as_deref()).await?;
    Ok(JsTtsSynthesis {
        pcm: Buffer::from(pcm),
        sample_rate: OUT_SAMPLE_RATE,
        channels: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use node_webrtc_rust_speech::config::TtsVendor;
    use node_webrtc_rust_speech::error::SpeechResult;
    use node_webrtc_rust_speech::pipeline::TtsAudioChunk;

    fn mock_config(api_key: Option<&str>) -> TtsConfig {
        TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: api_key.map(Into::into),
            endpoint: None,
            sentence_case: Default::default(),
            transport: Default::default(),
        }
    }

    struct Slow;
    #[async_trait]
    impl TtsProvider for Slow {
        fn vendor_name(&self) -> &'static str {
            "slow"
        }
        async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(vec![])
        }
    }

    struct Failing(String);
    #[async_trait]
    impl TtsProvider for Failing {
        fn vendor_name(&self) -> &'static str {
            "failing"
        }
        async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            Err(SpeechError::Vendor {
                vendor: "failing".into(),
                message: self.0.clone(),
            })
        }
    }

    struct Silent;
    #[async_trait]
    impl TtsProvider for Silent {
        fn vendor_name(&self) -> &'static str {
            "silent"
        }
        async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            Ok(vec![TtsAudioChunk {
                pcm: Bytes::new(),
                duration_ms: 0,
            }])
        }
    }

    #[tokio::test]
    async fn mock_vendor_returns_pcm() {
        let provider = default_vendor_registry()
            .create_tts(&mock_config(None))
            .unwrap();
        let out = run_synthesis(
            provider.as_ref(),
            "hello there",
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        assert!(!out.is_empty());
        // 24 kHz mono s16le is 48 bytes per ms; the mock makes "hello there" 550 ms.
        assert_eq!(out.len(), 550 * 48);
        assert_eq!(OUT_SAMPLE_RATE, 24_000);
    }

    #[tokio::test]
    async fn empty_and_long_text_are_invalid_input() {
        let provider = default_vendor_registry()
            .create_tts(&mock_config(None))
            .unwrap();
        for text in ["", "   ", &"x".repeat(1001)] {
            let e = run_synthesis(provider.as_ref(), text, Duration::from_secs(5), None)
                .await
                .err()
                .unwrap();
            assert_eq!(e.code, SynthesizeCode::InvalidInput);
        }
    }

    #[tokio::test]
    async fn slow_vendor_times_out() {
        let e = run_synthesis(&Slow, "hello", Duration::from_millis(50), None)
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, SynthesizeCode::VendorTimeout);
    }

    #[tokio::test]
    async fn status_codes_map_to_codes() {
        let cases = [
            (
                "speech HTTP 429 Too Many Requests: slow down",
                SynthesizeCode::VendorRateLimited,
            ),
            (
                "speech HTTP 401 Unauthorized: bad",
                SynthesizeCode::VendorKeyInvalid,
            ),
            (
                "speech HTTP 403 Forbidden",
                SynthesizeCode::VendorKeyInvalid,
            ),
            (
                "speech HTTP 500 Internal Server Error",
                SynthesizeCode::VendorError,
            ),
            (
                "speech HTTP 500 Internal Server Error: request 9a-401-bc",
                SynthesizeCode::VendorError,
            ),
            (
                "{\"err_code\":\"INVALID_AUTH\",\"request_id\":\"x-403-y\"}",
                SynthesizeCode::VendorError,
            ),
            ("401 Unauthorized: bad", SynthesizeCode::VendorKeyInvalid),
            ("429: slow down", SynthesizeCode::VendorRateLimited),
            (
                "fishaudio http 429: slow down",
                SynthesizeCode::VendorRateLimited,
            ),
            (
                "HTTP error: 429 Too Many Requests",
                SynthesizeCode::VendorRateLimited,
            ),
        ];
        for (message, want) in cases {
            let e = run_synthesis(&Failing(message.into()), "hi", Duration::from_secs(5), None)
                .await
                .err()
                .unwrap();
            assert_eq!(e.code, want, "{message}");
        }
    }

    #[tokio::test]
    async fn error_message_does_not_contain_the_api_key() {
        let key = "sk-test-SECRET-123";
        let message =
            format!("speech HTTP 401 Unauthorized: bad key {key} Authorization: Bearer {key}");
        let e = run_synthesis(&Failing(message), "hi", Duration::from_secs(5), Some(key))
            .await
            .err()
            .unwrap();
        assert!(!e.message.contains(key), "{}", e.message);
        assert!(e.message.contains("[redacted]"));
    }

    #[test]
    fn redact_strips_bearer_tokens_without_a_known_key() {
        let out = redact("header Authorization: Bearer abc.def-123 end", None);
        assert!(!out.contains("abc.def-123"), "{out}");
        assert!(out.contains("end"));
    }

    #[tokio::test]
    async fn empty_audio_is_a_vendor_error() {
        let e = run_synthesis(&Silent, "hi", Duration::from_secs(5), None)
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, SynthesizeCode::VendorError);
    }

    struct Odd;
    #[async_trait]
    impl TtsProvider for Odd {
        fn vendor_name(&self) -> &'static str {
            "odd"
        }
        async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            // 3 stereo frames (12 bytes) per chunk.
            let chunk = TtsAudioChunk {
                pcm: Bytes::from(vec![0u8; 12]),
                duration_ms: 0,
            };
            Ok(vec![chunk.clone(), chunk])
        }
    }

    #[tokio::test]
    async fn odd_frame_chunks_keep_every_sample_pair() {
        let out = run_synthesis(&Odd, "hi", Duration::from_secs(5), None)
            .await
            .unwrap();
        // 6 stereo frames -> 3 mono 24 kHz samples (2 bytes each).
        assert_eq!(out.len(), 6);
    }
}
