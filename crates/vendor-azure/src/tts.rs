//! Azure TTS — documented REST SSML synthesis, raw PCM body forwarded to the sink as it arrives.

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
#[cfg(feature = "live")]
use node_webrtc_rust_speech::pcm::{duration_ms_from_stereo_48k, MonoToStereo48k};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

#[cfg(feature = "live")]
use crate::factory::speech_key_from;
#[cfg(feature = "live")]
use crate::matrix::tts_rest_url;
use crate::matrix::{default_tts_voice, validate_tts_voice};

/// Raw 48 kHz 16-bit mono PCM: the agent's rate, so no resampling, and a higher rate than the
/// 16 kHz WAV this crate asked for before. Listed under "Streaming" audio outputs.
#[cfg(feature = "live")]
const OUTPUT_FORMAT: &str = "raw-48khz-16bit-mono-pcm";
#[cfg(feature = "live")]
const AZURE_TTS_PCM_RATE: u32 = 48_000;

pub struct AzureTts {
    speech_key: Option<String>,
    region: String,
    voice: String,
    /// One client for the provider's life: connections (and TLS sessions) are reused.
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// First-audio and between-chunk bounds of the body stream.
    #[cfg(feature = "live")]
    bounds: (std::time::Duration, std::time::Duration),
    #[cfg(feature = "live")]
    url_override: Option<String>,
    last_transport: std::sync::Mutex<Option<String>>,
}

impl AzureTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        crate::matrix::check_tts_transport(config.transport)?;
        let voice = config
            .voice
            .clone()
            .unwrap_or_else(|| default_tts_voice().to_string());
        validate_tts_voice(&voice).map_err(SpeechError::Config)?;
        let region = resolve_region(config)?;
        Ok(Self {
            #[cfg(feature = "live")]
            http: crate::http::build_client(None)?,
            #[cfg(feature = "live")]
            bounds: (
                crate::matrix::FIRST_AUDIO_TIMEOUT,
                crate::matrix::CHUNK_IDLE_TIMEOUT,
            ),
            #[cfg(feature = "live")]
            url_override: None,
            last_transport: std::sync::Mutex::new(None),
            speech_key: config.api_key.clone().or_else(|| {
                ["AZURE_SPEECH_KEY", "SPEECH_KEY"]
                    .into_iter()
                    .find_map(|env| std::env::var(env).ok())
            }),
            region,
            voice,
        })
    }
}

#[cfg(feature = "live")]
impl AzureTts {
    /// Test hook: POST here instead of the regional host (plain `http://` mock server).
    #[doc(hidden)]
    pub fn __with_url_override(mut self, url: impl Into<String>) -> Self {
        self.url_override = Some(url.into());
        self
    }

    /// Test hook: replaces the first-audio and between-chunk bounds of the body stream.
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
    #[doc(hidden)]
    pub fn __with_total_timeout(mut self, total: std::time::Duration) -> Self {
        self.http = crate::http::build_client(Some(total)).expect("test http client");
        self
    }
}

fn resolve_region(config: &TtsConfig) -> SpeechResult<String> {
    if let Some(endpoint) = config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(region) = parse_region_from_tts_endpoint(endpoint) {
            return Ok(region);
        }
    }
    std::env::var("AZURE_SPEECH_REGION").map_err(|_| {
        SpeechError::Config(
            "Azure TTS requires AZURE_SPEECH_REGION or config.endpoint (regional TTS host)".into(),
        )
    })
}

fn parse_region_from_tts_endpoint(endpoint: &str) -> Option<String> {
    let host = endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let prefix = host.strip_suffix(".tts.speech.microsoft.com")?;
    Some(prefix.to_string())
}

#[async_trait]
impl TtsProvider for AzureTts {
    fn vendor_name(&self) -> &'static str {
        "azure"
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

impl AzureTts {
    /// `POST /cognitiveservices/v1` with `X-Microsoft-OutputFormat: raw-48khz-16bit-mono-pcm`.
    /// Microsoft lists that format under "Streaming" audio outputs and says of a `200` answer:
    /// "This file can be played as it's transferred, saved to a buffer, or saved to a file."
    /// The body is forwarded to `sink` as it arrives, each chunk once; the return value is the
    /// same audio. A body error after audio reached the sink is an error and nothing is retried
    /// (`one-send-path-per-unit.md`). A body without audio is an error.
    async fn synthesize_streamed(
        &self,
        text: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let (first_audio_timeout, chunk_idle_timeout) = self.bounds;
            let key = speech_key_from(&self.speech_key)?;
            let url = self
                .url_override
                .clone()
                .unwrap_or_else(|| tts_rest_url(&self.region));
            let ssml = format!(
                "<speak version='1.0' xml:lang='en-US'><voice name='{}'>{}</voice></speak>",
                self.voice,
                xml_escape(text)
            );
            let first_audio_deadline = tokio::time::Instant::now() + first_audio_timeout;
            let first_audio_timed_out = || {
                vendor_err(format!(
                    "azure tts first audio timed out after {} ms",
                    first_audio_timeout.as_millis()
                ))
            };
            let mut response = tokio::time::timeout_at(
                first_audio_deadline,
                self.http
                    .post(url)
                    .header("Ocp-Apim-Subscription-Key", key)
                    .header("Content-Type", "application/ssml+xml")
                    .header("X-Microsoft-OutputFormat", OUTPUT_FORMAT)
                    // Documented as a required header of the text to speech request.
                    .header("User-Agent", "node-webrtc-rust")
                    .body(ssml)
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
                let text = response.text().await.unwrap_or_default();
                return Err(vendor_err(format!("{status}: {text}")));
            }

            let mut out: Vec<TtsAudioChunk> = Vec::new();
            // 48000 Hz is a passthrough that only duplicates the channel.
            let mut up = MonoToStereo48k::new(AZURE_TTS_PCM_RATE)?;
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
                                "azure tts stalled after {} ms",
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
                let mut bytes = chunk.to_vec();
                if let Some(first) = carry.take() {
                    bytes.insert(0, first);
                }
                if bytes.len() % 2 == 1 {
                    carry = bytes.pop();
                }
                if bytes.is_empty() {
                    continue;
                }
                if let Some(audio) = audio_chunk(up.push(&bytes)) {
                    if let Some(s) = sink {
                        let _ = s.send(audio.clone());
                    }
                    out.push(audio);
                }
            }
            if !sink.is_some_and(TtsProgressiveSink::is_cancelled) {
                if let Some(audio) = audio_chunk(up.finish()) {
                    if let Some(s) = sink {
                        let _ = s.send(audio.clone());
                    }
                    out.push(audio);
                }
            }
            if out.is_empty() {
                return Err(vendor_err("Azure TTS returned no audio"));
            }
            Ok(out)
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = (text, sink);
            Err(SpeechError::Vendor {
                vendor: "azure".into(),
                message: "live Azure TTS requires `--features live` on vendor-azure".into(),
            })
        }
    }
}

/// 48 kHz stereo bytes as a sink chunk; `None` for no bytes.
#[cfg(feature = "live")]
fn audio_chunk(stereo: Vec<u8>) -> Option<TtsAudioChunk> {
    if stereo.is_empty() {
        return None;
    }
    Some(TtsAudioChunk {
        duration_ms: duration_ms_from_stereo_48k(stereo.len()),
        pcm: bytes::Bytes::from(stereo),
    })
}

#[cfg(feature = "live")]
fn vendor_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "azure".into(),
        message: message.into(),
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn rejects_undocumented_voice() {
        assert!(matches!(
            AzureTts::new(&TtsConfig {
                provider: TtsVendor::Azure,
                model: None,
                model_path: None,
                voice: Some("unknown".into()),
                api_key: Some("k".into()),
                endpoint: Some("eastus.tts.speech.microsoft.com".into()),
                sentence_case: Default::default(),
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
