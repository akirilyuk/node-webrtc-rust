#[cfg(feature = "live")]
use async_openai::types::{CreateSpeechRequestArgs, SpeechModel, SpeechResponseFormat, Voice};
#[cfg(feature = "live")]
use async_openai::Client;
use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
#[cfg(feature = "live")]
use tokio::sync::Mutex;

use crate::factory::api_key_from;

/// OpenAI TTS PCM output sample rate (16-bit mono LE).
const OPENAI_TTS_PCM_SAMPLE_RATE: u32 = 24_000;
const DEFAULT_API_ROOT: &str = "https://api.openai.com/v1";

/// How this vendor instance turns text into PCM.
///
/// Chosen once from `GET /v1/models/{model}` during [`TtsProvider::prepare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeechDelivery {
    /// Chunked raw PCM (`stream_format: audio`).
    StreamPcm,
    /// Buffer the whole `/audio/speech` body, then emit.
    FullBody,
}

pub struct OpenAiTts {
    api_key: Option<String>,
    model: String,
    voice: String,
    endpoint: Option<String>,
    /// `None` until the one-time model lookup finishes.
    #[cfg(feature = "live")]
    delivery: Mutex<Option<SpeechDelivery>>,
}

impl OpenAiTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            model: config.model.clone().unwrap_or_else(|| "tts-1".to_string()),
            voice: config.voice.clone().unwrap_or_else(|| "alloy".to_string()),
            endpoint: config.endpoint.clone(),
            #[cfg(feature = "live")]
            delivery: Mutex::new(None),
        })
    }
}

#[async_trait]
impl TtsProvider for OpenAiTts {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    async fn prepare(&self) -> SpeechResult<()> {
        #[cfg(feature = "live")]
        {
            match self.lookup_and_store().await {
                Ok(()) => {}
                // A blip on the models endpoint must not block session start.
                // The first utterance retries the lookup.
                Err(err) if !is_fatal_lookup(&err) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_full_body(text).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let mode = self.delivery_mode().await?;
            match mode {
                SpeechDelivery::FullBody => {
                    let chunks = self.synthesize_full_body(text).await?;
                    emit_chunks(sink.as_ref(), &chunks);
                    Ok(chunks)
                }
                SpeechDelivery::StreamPcm => self.synthesize_pcm_stream(text, sink).await,
            }
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = sink;
            self.synthesize_full_body(text).await
        }
    }
}

impl OpenAiTts {
    async fn synthesize_full_body(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let api_key = api_key_from(&self.api_key, "OPENAI_API_KEY")?;
            let request = CreateSpeechRequestArgs::default()
                .input(text.to_string())
                .model(parse_speech_model(&self.model))
                .voice(parse_voice(&self.voice))
                .response_format(SpeechResponseFormat::Pcm)
                .build()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: err.to_string(),
                })?;

            let mut config = async_openai::config::OpenAIConfig::new().with_api_key(api_key);
            if self
                .endpoint
                .as_deref()
                .is_some_and(|endpoint| !endpoint.trim().is_empty())
            {
                config = config.with_api_base(api_root(&self.endpoint));
            }
            let client = Client::with_config(config);
            let response =
                client
                    .audio()
                    .speech(request)
                    .await
                    .map_err(|err| SpeechError::Vendor {
                        vendor: "openai".into(),
                        message: err.to_string(),
                    })?;

            let mono_24k = response.bytes;
            let duration_ms =
                duration_ms_from_mono_s16le(mono_24k.len(), OPENAI_TTS_PCM_SAMPLE_RATE);
            let pcm = mono_24k_s16le_to_stereo_48k(mono_24k.as_ref());
            Ok(vec![TtsAudioChunk { pcm, duration_ms }])
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, api_key_from(&self.api_key, "OPENAI_API_KEY"));
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI TTS requires `--features live` on vendor-openai".into(),
            })
        }
    }

    #[cfg(feature = "live")]
    async fn delivery_mode(&self) -> SpeechResult<SpeechDelivery> {
        self.lookup_and_store().await?;
        self.delivery
            .lock()
            .await
            .ok_or_else(|| SpeechError::Vendor {
                vendor: "openai".into(),
                message: "TTS delivery mode was not resolved".into(),
            })
    }

    #[cfg(feature = "live")]
    async fn lookup_and_store(&self) -> SpeechResult<()> {
        if self.delivery.lock().await.is_some() {
            return Ok(());
        }
        let mode = self.fetch_model_delivery().await?;
        *self.delivery.lock().await = Some(mode);
        Ok(())
    }

    /// `GET /v1/models/{model}` once. Explicit capability flags win. The published
    /// model object has no speech flags, and chunked PCM (`stream_format: audio`)
    /// is the speech API default for every TTS model, so that is the choice when
    /// the payload does not say otherwise. A later stream rejection flips this
    /// instance to [`SpeechDelivery::FullBody`].
    #[cfg(feature = "live")]
    async fn fetch_model_delivery(&self) -> SpeechResult<SpeechDelivery> {
        let api_key = api_key_from(&self.api_key, "OPENAI_API_KEY")?;
        let url = format!(
            "{}/models/{}",
            api_root(&self.endpoint),
            encode_model_id(&self.model)
        );
        let http = reqwest::Client::builder()
            .build()
            .map_err(|err| SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("HTTP client: {err}"),
            })?;
        let response = http
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("model lookup failed: {err}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("model lookup HTTP {}: {body}", status.as_u16()),
            });
        }
        let body: serde_json::Value = response.json().await.map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: format!("model lookup JSON: {err}"),
        })?;
        Ok(speech_delivery_from_model_body(&body))
    }

    #[cfg(feature = "live")]
    async fn synthesize_pcm_stream(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        match self.read_pcm_stream(text, sink.as_ref()).await {
            Ok(chunks) => Ok(chunks),
            Err(StreamAttempt::Rejected) => {
                *self.delivery.lock().await = Some(SpeechDelivery::FullBody);
                let chunks = self.synthesize_full_body(text).await?;
                emit_chunks(sink.as_ref(), &chunks);
                Ok(chunks)
            }
            Err(StreamAttempt::Failed(err)) => Err(err),
        }
    }

    #[cfg(feature = "live")]
    async fn read_pcm_stream(
        &self,
        text: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> Result<Vec<TtsAudioChunk>, StreamAttempt> {
        use futures_util::StreamExt;

        let api_key =
            api_key_from(&self.api_key, "OPENAI_API_KEY").map_err(StreamAttempt::Failed)?;
        let url = format!("{}/audio/speech", api_root(&self.endpoint));
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": self.voice,
            "response_format": "pcm",
            "stream_format": "audio",
        });
        let http = reqwest::Client::builder().build().map_err(|err| {
            StreamAttempt::Failed(SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("HTTP client: {err}"),
            })
        })?;
        let response = http
            .post(url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                StreamAttempt::Failed(SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: format!("speech stream: {err}"),
                })
            })?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() || content_type.contains("application/json") {
            let text_body = response.text().await.unwrap_or_default();
            if stream_format_rejected(status.as_u16(), &text_body) {
                return Err(StreamAttempt::Rejected);
            }
            return Err(StreamAttempt::Failed(SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("speech HTTP {}: {text_body}", status.as_u16()),
            }));
        }

        let mut pending = Vec::new();
        let mut chunks = Vec::new();
        let mut forward = true;
        let mut stream = response.bytes_stream();
        while let Some(next) = stream.next().await {
            let bytes = next.map_err(|err| {
                StreamAttempt::Failed(SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: format!("speech stream read: {err}"),
                })
            })?;
            pending.extend_from_slice(&bytes);
            let aligned = take_even_s16le(&mut pending);
            if aligned.is_empty() {
                continue;
            }
            let duration_ms =
                duration_ms_from_mono_s16le(aligned.len(), OPENAI_TTS_PCM_SAMPLE_RATE);
            let pcm = mono_24k_s16le_to_stereo_48k(&aligned);
            let chunk = TtsAudioChunk { pcm, duration_ms };
            if forward {
                if let Some(sink) = sink {
                    if !sink.send(chunk.clone()) {
                        forward = false;
                    }
                }
            }
            chunks.push(chunk);
        }
        if chunks.is_empty() {
            return Err(StreamAttempt::Failed(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "speech stream returned no PCM".into(),
            }));
        }
        Ok(chunks)
    }
}

#[cfg(feature = "live")]
enum StreamAttempt {
    /// API refused `stream_format`; caller switches this instance to full-body.
    Rejected,
    Failed(SpeechError),
}

fn emit_chunks(sink: Option<&TtsProgressiveSink>, chunks: &[TtsAudioChunk]) {
    if let Some(sink) = sink {
        for chunk in chunks {
            if !sink.send(chunk.clone()) {
                break;
            }
        }
    }
}

/// Pop a whole number of 16-bit samples, leaving a trailing odd byte in `buf`.
fn take_even_s16le(buf: &mut Vec<u8>) -> Vec<u8> {
    let even = buf.len() - (buf.len() % 2);
    let out = buf[..even].to_vec();
    buf.drain(..even);
    out
}

fn api_root(endpoint: &Option<String>) -> String {
    endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_API_ROOT)
        .trim_end_matches('/')
        .to_string()
}

fn encode_model_id(model: &str) -> String {
    let mut out = String::with_capacity(model.len());
    for byte in model.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Map a `GET /v1/models/{model}` body to a delivery mode.
///
/// Explicit flags (`capabilities.speech.stream_formats`, `capabilities.streaming`,
/// `capabilities.tts_audio_stream`) decide when present. Without them, chunked
/// PCM is selected because that is the documented default for the speech API.
fn speech_delivery_from_model_body(body: &serde_json::Value) -> SpeechDelivery {
    delivery_from_capabilities(body).unwrap_or(SpeechDelivery::StreamPcm)
}

fn delivery_from_capabilities(body: &serde_json::Value) -> Option<SpeechDelivery> {
    let caps = body.get("capabilities")?;
    if caps
        .get("tts_audio_stream")
        .and_then(|value| value.as_bool())
        == Some(false)
    {
        return Some(SpeechDelivery::FullBody);
    }
    if caps.get("streaming").and_then(|value| value.as_bool()) == Some(false)
        && caps.get("speech").is_none()
        && caps.get("tts_audio_stream").is_none()
    {
        return Some(SpeechDelivery::FullBody);
    }
    if let Some(speech) = caps.get("speech") {
        if let Some(formats) = speech
            .get("stream_formats")
            .and_then(|value| value.as_array())
        {
            let has_audio = formats.iter().any(|value| value.as_str() == Some("audio"));
            return Some(if has_audio {
                SpeechDelivery::StreamPcm
            } else {
                SpeechDelivery::FullBody
            });
        }
        if speech.get("stream_audio").and_then(|value| value.as_bool()) == Some(false) {
            return Some(SpeechDelivery::FullBody);
        }
        if speech.get("stream_audio").and_then(|value| value.as_bool()) == Some(true) {
            return Some(SpeechDelivery::StreamPcm);
        }
    }
    if caps
        .get("tts_audio_stream")
        .and_then(|value| value.as_bool())
        == Some(true)
        || caps.get("streaming").and_then(|value| value.as_bool()) == Some(true)
    {
        return Some(SpeechDelivery::StreamPcm);
    }
    None
}

fn stream_format_rejected(status: u16, body: &str) -> bool {
    // 200 is included because a JSON error body can still arrive with that status
    // when `Content-Type` is `application/json` (checked by the caller).
    if !matches!(status, 200 | 400 | 404 | 422) {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("stream_format") || lower.contains("stream format")
}

fn is_fatal_lookup(err: &SpeechError) -> bool {
    let SpeechError::Vendor { message, .. } = err else {
        return false;
    };
    message.contains("HTTP 401") || message.contains("HTTP 403") || message.contains("HTTP 404")
}

#[cfg(feature = "live")]
fn parse_voice(voice: &str) -> Voice {
    match voice {
        "alloy" => Voice::Alloy,
        "ash" => Voice::Ash,
        "coral" => Voice::Coral,
        "echo" => Voice::Echo,
        "fable" => Voice::Fable,
        "onyx" => Voice::Onyx,
        "nova" => Voice::Nova,
        "sage" => Voice::Sage,
        "shimmer" => Voice::Shimmer,
        _ => Voice::Alloy,
    }
}

#[cfg(feature = "live")]
fn parse_speech_model(model: &str) -> SpeechModel {
    match model {
        "tts-1-hd" => SpeechModel::Tts1Hd,
        other => SpeechModel::Other(other.to_string()),
    }
}

/// Upsample mono 24 kHz s16le to stereo 48 kHz for WebRTC outbound tracks.
fn mono_24k_s16le_to_stereo_48k(mono_24k: &[u8]) -> bytes::Bytes {
    let mut mono_48k = Vec::with_capacity(mono_24k.len() * 2);
    for sample in mono_24k.chunks_exact(2) {
        mono_48k.extend_from_slice(sample);
        mono_48k.extend_from_slice(sample);
    }
    mono_s16le_to_stereo(&mono_48k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn upsample_doubles_byte_length_before_stereo() {
        let mono_24k = vec![0_u8; 480];
        let stereo_48k = mono_24k_s16le_to_stereo_48k(&mono_24k);
        assert_eq!(stereo_48k.len(), 480 * 2 * 2);
    }

    #[test]
    fn take_even_s16le_keeps_trailing_odd_byte() {
        let mut buf = vec![1, 2, 3];
        let even = take_even_s16le(&mut buf);
        assert_eq!(even, vec![1, 2]);
        assert_eq!(buf, vec![3]);
    }

    #[test]
    fn model_body_without_capabilities_streams_pcm() {
        let body = serde_json::json!({
            "id": "tts-1",
            "object": "model",
            "created": 0,
            "owned_by": "openai"
        });
        assert_eq!(
            speech_delivery_from_model_body(&body),
            SpeechDelivery::StreamPcm
        );
    }

    #[test]
    fn capabilities_audio_stream_format_selects_stream() {
        let body = serde_json::json!({
            "id": "tts-1",
            "capabilities": { "speech": { "stream_formats": ["audio"] } }
        });
        assert_eq!(
            speech_delivery_from_model_body(&body),
            SpeechDelivery::StreamPcm
        );
    }

    #[test]
    fn capabilities_without_audio_select_full_body() {
        let sse_only = serde_json::json!({
            "capabilities": { "speech": { "stream_formats": ["sse"] } }
        });
        assert_eq!(
            speech_delivery_from_model_body(&sse_only),
            SpeechDelivery::FullBody
        );
        let disabled = serde_json::json!({
            "capabilities": { "tts_audio_stream": false }
        });
        assert_eq!(
            speech_delivery_from_model_body(&disabled),
            SpeechDelivery::FullBody
        );
    }

    #[test]
    fn stream_format_rejection_is_narrow() {
        assert!(stream_format_rejected(
            400,
            r#"{"error":{"param":"stream_format","message":"not supported"}}"#
        ));
        assert!(!stream_format_rejected(
            400,
            r#"{"error":{"param":"voice","message":"invalid voice"}}"#
        ));
        assert!(stream_format_rejected(
            200,
            r#"{"error":{"param":"stream_format"}}"#
        ));
        assert!(!stream_format_rejected(500, "stream_format"));
    }

    #[test]
    fn fatal_lookup_is_auth_or_missing_model() {
        let missing = SpeechError::Vendor {
            vendor: "openai".into(),
            message: "model lookup HTTP 404: missing".into(),
        };
        let blip = SpeechError::Vendor {
            vendor: "openai".into(),
            message: "model lookup failed: connection reset".into(),
        };
        assert!(is_fatal_lookup(&missing));
        assert!(!is_fatal_lookup(&blip));
    }

    #[cfg(feature = "live")]
    #[test]
    fn parse_voice_and_model() {
        assert!(matches!(parse_voice("alloy"), Voice::Alloy));
        assert!(matches!(
            parse_speech_model("tts-1-hd"),
            SpeechModel::Tts1Hd
        ));
    }

    #[test]
    fn factory_defaults() {
        let tts = OpenAiTts::new(&TtsConfig {
            provider: TtsVendor::Openai,
            model: None,
            model_path: None,
            voice: None,
            api_key: Some("test".into()),
            endpoint: None,
        })
        .unwrap();
        assert_eq!(tts.model, "tts-1");
        assert_eq!(tts.voice, "alloy");
        assert_eq!(api_root(&tts.endpoint), DEFAULT_API_ROOT);
    }
}
