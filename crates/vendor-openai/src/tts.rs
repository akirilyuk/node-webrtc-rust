#[cfg(feature = "live")]
use async_openai::types::{CreateSpeechRequestArgs, SpeechModel, SpeechResponseFormat, Voice};
#[cfg(feature = "live")]
use async_openai::Client;
#[cfg(feature = "live")]
use base64::Engine;
#[cfg(feature = "live")]
use serde_json::Value;
use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
#[cfg(feature = "live")]
use tokio::sync::Mutex;

use crate::factory::api_key_from;
use crate::matrix::{tts_delivery_plan, tts_sse_allowed, TtsStreamFormat};

/// OpenAI TTS PCM output sample rate (16-bit mono LE).
const OPENAI_TTS_PCM_SAMPLE_RATE: u32 = 24_000;
const DEFAULT_API_ROOT: &str = "https://api.openai.com/v1";

/// How this vendor instance turns text into PCM.
///
/// Starts as chunked PCM. If that request fails before any audio is forwarded,
/// this instance uses the buffered speech call from then on.
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
    /// Chunked PCM until a stream attempt fails before any audio is sent.
    #[cfg(feature = "live")]
    delivery: Mutex<SpeechDelivery>,
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
            delivery: Mutex::new(SpeechDelivery::StreamPcm),
        })
    }
}

#[async_trait]
impl TtsProvider for OpenAiTts {
    fn vendor_name(&self) -> &'static str {
        "openai"
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
            let mode = *self.delivery.lock().await;
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
    async fn synthesize_pcm_stream(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        match self.read_pcm_stream(text, sink.as_ref()).await {
            Ok(chunks) => Ok(chunks),
            Err(StreamAttempt::UseFullBody) => {
                *self.delivery.lock().await = SpeechDelivery::FullBody;
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
            .map_err(|_| StreamAttempt::UseFullBody)?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() || content_type.contains("application/json") {
            return Err(StreamAttempt::UseFullBody);
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
            return Err(StreamAttempt::UseFullBody);
        }
        Ok(chunks)
    }

    /// `stream_format=sse` per OpenAPI `CreateSpeechResponseStreamEvent`
    /// (`speech.audio.delta` / `speech.audio.done`). Not for VoiceAgent default.
    #[cfg(feature = "live")]
    pub async fn synthesize_pcm_sse(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        if !tts_sse_allowed(&self.model) {
            return Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!(
                    "stream_format=sse is not supported for OpenAI TTS model `{}`",
                    self.model
                ),
            });
        }
        use futures_util::StreamExt;

        let api_key = api_key_from(&self.api_key, "OPENAI_API_KEY")?;
        let url = format!("{}/audio/speech", api_root(&self.endpoint));
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": self.voice,
            "response_format": "pcm",
            "stream_format": "sse",
        });
        let http = reqwest::Client::builder().build().map_err(|err| {
            SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("HTTP client: {err}"),
            }
        })?;
        let response = http
            .post(url)
            .bearer_auth(api_key)
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "openai".into(),
                message: err.to_string(),
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("speech SSE HTTP {}: {}", status, body),
            });
        }

        let mut pending_pcm = Vec::new();
        let mut chunks = Vec::new();
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(next) = stream.next().await {
            let bytes = next.map_err(|err| SpeechError::Vendor {
                vendor: "openai".into(),
                message: format!("speech SSE read: {err}"),
            })?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(pos) = buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n")) {
                let sep_len = if buffer.get(pos..pos + 4) == Some("\r\n\r\n") {
                    4
                } else {
                    2
                };
                let frame = buffer[..pos].to_string();
                buffer.drain(..pos + sep_len);
                for line in frame.lines() {
                    let line = line.trim();
                    if !line.starts_with("data:") {
                        continue;
                    }
                    let payload = line.trim_start_matches("data:").trim();
                    if payload.is_empty() || payload == "[DONE]" {
                        continue;
                    }
                    let event: Value = serde_json::from_str(payload).map_err(|err| {
                        SpeechError::Vendor {
                            vendor: "openai".into(),
                            message: format!("speech SSE JSON: {err}"),
                        }
                    })?;
                    match event.get("type").and_then(Value::as_str) {
                        Some("speech.audio.delta") => {
                            let audio_b64 = event
                                .get("audio")
                                .and_then(Value::as_str)
                                .ok_or_else(|| SpeechError::Vendor {
                                    vendor: "openai".into(),
                                    message: "speech.audio.delta missing audio".into(),
                                })?;
                            let decoded = base64::engine::general_purpose::STANDARD
                                .decode(audio_b64)
                                .map_err(|err| SpeechError::Vendor {
                                    vendor: "openai".into(),
                                    message: format!("speech.audio.delta base64: {err}"),
                                })?;
                            pending_pcm.extend_from_slice(&decoded);
                            let aligned = take_even_s16le(&mut pending_pcm);
                            if aligned.is_empty() {
                                continue;
                            }
                            let duration_ms = duration_ms_from_mono_s16le(
                                aligned.len(),
                                OPENAI_TTS_PCM_SAMPLE_RATE,
                            );
                            let pcm = mono_24k_s16le_to_stereo_48k(&aligned);
                            chunks.push(TtsAudioChunk { pcm, duration_ms });
                        }
                        Some("speech.audio.done") => {}
                        _ => {}
                    }
                }
            }
        }
        let aligned = take_even_s16le(&mut pending_pcm);
        if !aligned.is_empty() {
            let duration_ms =
                duration_ms_from_mono_s16le(aligned.len(), OPENAI_TTS_PCM_SAMPLE_RATE);
            let pcm = mono_24k_s16le_to_stereo_48k(&aligned);
            chunks.push(TtsAudioChunk { pcm, duration_ms });
        }
        if chunks.is_empty() {
            return Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "speech SSE stream produced no PCM".into(),
            });
        }
        Ok(chunks)
    }
}

#[cfg(feature = "live")]
pub(crate) fn parse_speech_sse_event_for_test(event: &Value) -> Option<Vec<u8>> {
    if event.get("type").and_then(Value::as_str) != Some("speech.audio.delta") {
        return None;
    }
    let audio_b64 = event.get("audio").and_then(Value::as_str)?;
    base64::engine::general_purpose::STANDARD
        .decode(audio_b64)
        .ok()
}

#[cfg(feature = "live")]
enum StreamAttempt {
    /// Nothing was forwarded yet. Caller uses the buffered speech call.
    UseFullBody,
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
    fn tts_matrix_forbids_sse_on_tts1() {
        assert!(!tts_sse_allowed("tts-1"));
        let plan = tts_delivery_plan("tts-1");
        assert_eq!(plan.try_stream, TtsStreamFormat::Audio);
    }

    #[cfg(feature = "live")]
    #[test]
    fn parse_speech_audio_delta_openapi_shape() {
        use base64::Engine;
        use serde_json::json;
        let pcm = vec![1_u8, 2, 3, 4];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm);
        let event = json!({
            "type": "speech.audio.delta",
            "audio": b64
        });
        assert_eq!(parse_speech_sse_event_for_test(&event), Some(pcm));
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
