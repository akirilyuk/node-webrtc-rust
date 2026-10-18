//! Groq TTS — documented `POST /openai/v1/audio/speech`, WAV body forwarded to the sink as it arrives.

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
#[cfg(feature = "live")]
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

#[cfg(feature = "live")]
use crate::factory::api_key_from;
use crate::matrix::{default_tts_model, validate_tts_model, DEFAULT_SPEECH_URL};
#[cfg(feature = "live")]
use crate::wav::WavStream;

pub struct GroqTts {
    api_key: Option<String>,
    speech_url: String,
    model: String,
    voice: String,
    /// One client for the provider's life: connections (and TLS sessions) are reused.
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// First-audio and between-chunk bounds of the body stream.
    #[cfg(feature = "live")]
    bounds: (std::time::Duration, std::time::Duration),
    last_transport: std::sync::Mutex<Option<String>>,
}

impl GroqTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        crate::matrix::check_tts_transport(config.transport)?;
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_tts_model().to_string());
        validate_tts_model(&model).map_err(SpeechError::Config)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("GROQ_API_KEY").ok()),
            speech_url: config
                .endpoint
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_SPEECH_URL.to_string()),
            model,
            voice: config.voice.clone().unwrap_or_else(|| "troy".to_string()),
            #[cfg(feature = "live")]
            http: crate::http::build_client(None)?,
            #[cfg(feature = "live")]
            bounds: (
                crate::matrix::FIRST_AUDIO_TIMEOUT,
                crate::matrix::CHUNK_IDLE_TIMEOUT,
            ),
            last_transport: std::sync::Mutex::new(None),
        })
    }

    /// Test hook: replaces the first-audio and between-chunk bounds of the body stream.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __with_bounds(
        mut self,
        first_audio: std::time::Duration,
        chunk_idle: std::time::Duration,
    ) -> Self {
        self.bounds = (first_audio, chunk_idle);
        self
    }

    /// Test hook: replaces the pooled client with one that has this whole-request bound.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __with_total_timeout(mut self, total: std::time::Duration) -> Self {
        self.http = crate::http::build_client(Some(total)).expect("test http client");
        self
    }
}

#[async_trait]
impl TtsProvider for GroqTts {
    fn vendor_name(&self) -> &'static str {
        "groq"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_streamed(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_streamed(text, sink.as_ref()).await
    }
}

impl GroqTts {
    /// `POST /openai/v1/audio/speech` with `response_format: wav`. Groq documents `flac, mp3,
    /// mulaw, ogg, wav` and no raw PCM, so the WAV header is parsed as the body arrives and the
    /// `data` payload is forwarded to `sink` chunk by chunk, each chunk once. The return value is
    /// the same audio. A body error after audio reached the sink is an error and nothing is
    /// retried (`one-send-path-per-unit.md`). A body without audio is an error.
    async fn synthesize_streamed(
        &self,
        text: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let (first_audio_timeout, chunk_idle_timeout) = self.bounds;
            let api_key = api_key_from(&self.api_key, "GROQ_API_KEY")?;
            let body = serde_json::json!({
                "model": self.model,
                "input": text,
                "voice": self.voice,
                "response_format": "wav",
            });
            let first_audio_deadline = tokio::time::Instant::now() + first_audio_timeout;
            let first_audio_timed_out = || {
                vendor_err(format!(
                    "groq tts first audio timed out after {} ms",
                    first_audio_timeout.as_millis()
                ))
            };
            let mut response = tokio::time::timeout_at(
                first_audio_deadline,
                self.http
                    .post(&self.speech_url)
                    .bearer_auth(api_key)
                    .json(&body)
                    .send(),
            )
            .await
            .map_err(|_| first_audio_timed_out())?
            .map_err(|err| vendor_err(err.to_string()))?;
            if let Ok(mut guard) = self.last_transport.lock() {
                *guard = Some(crate::http::transport_label(response.version()));
            }
            let status = response.status();
            if !status.is_success() {
                // The vendor's error text (for example "terms must be accepted" for Orpheus) is
                // part of the message; the key never is.
                let text = response.text().await.unwrap_or_default();
                return Err(vendor_err(format!("{status}: {text}")));
            }

            let mut wav = WavStream::new();
            let mut out: Vec<TtsAudioChunk> = Vec::new();
            // An odd trailing byte of one body chunk is the first half of the next sample.
            let mut carry: Option<u8> = None;
            loop {
                let next = if out.is_empty() {
                    tokio::time::timeout_at(first_audio_deadline, response.chunk())
                        .await
                        .map_err(|_| first_audio_timed_out())?
                } else {
                    tokio::time::timeout(chunk_idle_timeout, response.chunk())
                        .await
                        .map_err(|_| {
                            vendor_err(format!(
                                "groq tts stalled after {} ms",
                                chunk_idle_timeout.as_millis()
                            ))
                        })?
                };
                if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
                    break;
                }
                // A body that ends early or errors (reset, bad chunk framing, short
                // Content-Length) is an error, never partial success.
                let Some(chunk) = next.map_err(|err| vendor_err(err.to_string()))? else {
                    break;
                };
                let mut bytes = wav.feed(&chunk).map_err(|err| vendor_err(err.0))?;
                if let Some(first) = carry.take() {
                    bytes.insert(0, first);
                }
                if bytes.len() % 2 == 1 {
                    carry = bytes.pop();
                }
                if bytes.is_empty() {
                    continue;
                }
                let rate = wav.rate().expect("rate is known once data bytes arrive");
                let audio = agent_chunk(&bytes, rate).map_err(vendor_err)?;
                if let Some(s) = sink {
                    let _ = s.send(audio.clone());
                }
                out.push(audio);
            }
            if out.is_empty() {
                return Err(vendor_err(if wav.in_data() {
                    "Groq TTS returned no audio"
                } else {
                    "Groq TTS body ended before the WAV data chunk"
                }));
            }
            Ok(out)
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = (text, sink);
            Err(SpeechError::Vendor {
                vendor: "groq".into(),
                message: "live Groq TTS requires `--features live` on vendor-groq".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
fn vendor_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "groq".into(),
        message: message.into(),
    }
}

/// Mono s16le at `rate` Hz as the agent's 48 kHz stereo. Rates that divide 48 kHz are repeated
/// sample by sample (no resampler exists in this workspace for TTS); any other rate is refused.
#[cfg(feature = "live")]
fn agent_chunk(mono: &[u8], rate: u32) -> Result<TtsAudioChunk, String> {
    if rate == 0 || !WEBRTC_PCM_SAMPLE_RATE.is_multiple_of(rate) {
        return Err(format!(
            "Groq TTS WAV sample rate {rate} Hz does not divide 48000 Hz"
        ));
    }
    let factor = (WEBRTC_PCM_SAMPLE_RATE / rate) as usize;
    let duration_ms = duration_ms_from_mono_s16le(mono.len(), rate);
    let pcm = if factor == 1 {
        mono_s16le_to_stereo(mono)
    } else {
        let mut stretched = Vec::with_capacity(mono.len() * factor);
        for sample in mono.as_chunks::<2>().0 {
            for _ in 0..factor {
                stretched.extend_from_slice(sample);
            }
        }
        mono_s16le_to_stereo(&stretched)
    };
    Ok(TtsAudioChunk { pcm, duration_ms })
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn rejects_undocumented_tts_model() {
        assert!(matches!(
            GroqTts::new(&TtsConfig {
                provider: TtsVendor::Groq,
                model: Some("tts-1".into()),
                model_path: None,
                voice: None,
                api_key: Some("k".into()),
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
