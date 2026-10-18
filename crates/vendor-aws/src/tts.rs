//! AWS Polly `SynthesizeSpeech` PCM, `AudioStream` forwarded to the sink chunk by chunk.

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
#[cfg(feature = "live")]
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::matrix::{default_tts_voice, validate_tts_voice};

#[cfg(feature = "live")]
const POLLY_PCM_RATE: u32 = 16_000;

pub struct AwsTts {
    voice: String,
    /// One Polly client for the provider's life (built on first use): the SDK pools its
    /// connections per client.
    #[cfg(feature = "live")]
    client: tokio::sync::OnceCell<aws_sdk_polly::Client>,
    #[cfg(feature = "live")]
    bounds: (std::time::Duration, std::time::Duration),
    /// `sdk`: the AWS SDK does not expose the HTTP version of a Polly call.
    last_transport: std::sync::Mutex<Option<String>>,
}

impl AwsTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        crate::matrix::check_tts_transport(config.transport)?;
        let voice = config
            .voice
            .clone()
            .unwrap_or_else(|| default_tts_voice().to_string());
        validate_tts_voice(&voice).map_err(SpeechError::Config)?;
        Ok(Self {
            voice,
            #[cfg(feature = "live")]
            client: tokio::sync::OnceCell::new(),
            #[cfg(feature = "live")]
            bounds: (
                crate::matrix::FIRST_AUDIO_TIMEOUT,
                crate::matrix::CHUNK_IDLE_TIMEOUT,
            ),
            last_transport: std::sync::Mutex::new(None),
        })
    }
}

#[cfg(feature = "live")]
impl AwsTts {
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

    /// Test hook: talk to `endpoint_url` (a plain `http://` mock server) with fixed fake
    /// credentials, so the test never reads `~/.aws` or the environment for credentials.
    #[doc(hidden)]
    pub fn __with_mock_endpoint(self, endpoint_url: &str) -> Self {
        use aws_sdk_polly::config::{BehaviorVersion, Credentials, Region};
        let config = aws_sdk_polly::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("test", "test", None, None, "nwr-test"))
            .endpoint_url(endpoint_url)
            .timeout_config(crate::matrix::polly_timeouts())
            .build();
        self.client
            .set(aws_sdk_polly::Client::from_conf(config))
            .expect("client is built on first use");
        self
    }
}

#[async_trait]
impl TtsProvider for AwsTts {
    fn vendor_name(&self) -> &'static str {
        "aws"
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

impl AwsTts {
    /// `SynthesizeSpeech` with `OutputFormat: pcm`. The response element `AudioStream` is "Stream
    /// containing the synthesized speech", and the SDK hands it out as a byte stream: each chunk
    /// is forwarded to `sink` as it arrives, once; the return value is the same audio. A stream
    /// error after audio reached the sink is an error and nothing is retried
    /// (`one-send-path-per-unit.md`). A stream without audio is an error.
    async fn synthesize_streamed(
        &self,
        text: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let (first_audio_timeout, chunk_idle_timeout) = self.bounds;
            let voice_id = voice_id_from_name(&self.voice).map_err(SpeechError::Config)?;
            let client = self
                .client
                .get_or_init(|| async {
                    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                        .timeout_config(crate::matrix::polly_timeouts())
                        .load()
                        .await;
                    aws_sdk_polly::Client::new(&config)
                })
                .await;
            let first_audio_deadline = tokio::time::Instant::now() + first_audio_timeout;
            let first_audio_timed_out = || {
                vendor_err(format!(
                    "polly first audio timed out after {} ms",
                    first_audio_timeout.as_millis()
                ))
            };
            let mut output = tokio::time::timeout_at(
                first_audio_deadline,
                client
                    .synthesize_speech()
                    .text(text)
                    .voice_id(voice_id)
                    .output_format(aws_sdk_polly::types::OutputFormat::Pcm)
                    .sample_rate("16000")
                    .engine(aws_sdk_polly::types::Engine::Neural)
                    .send(),
            )
            .await
            .map_err(|_| first_audio_timed_out())?
            .map_err(|err| {
                vendor_err(aws_sdk_polly::error::DisplayErrorContext(&err).to_string())
            })?;
            if let Ok(mut guard) = self.last_transport.lock() {
                *guard = Some("sdk".into());
            }

            let mut out: Vec<TtsAudioChunk> = Vec::new();
            // An odd trailing byte of one body chunk is the first half of the next sample.
            let mut carry: Option<u8> = None;
            loop {
                let next = if out.is_empty() {
                    tokio::time::timeout_at(first_audio_deadline, output.audio_stream.try_next())
                        .await
                        .map_err(|_| first_audio_timed_out())?
                } else {
                    tokio::time::timeout(chunk_idle_timeout, output.audio_stream.try_next())
                        .await
                        .map_err(|_| {
                            vendor_err(format!(
                                "polly stream stalled after {} ms",
                                chunk_idle_timeout.as_millis()
                            ))
                        })?
                };
                if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
                    break;
                }
                // A stream that errors (reset, bad framing) is an error, never partial success.
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
                let audio = agent_chunk(&bytes);
                if let Some(s) = sink {
                    let _ = s.send(audio.clone());
                }
                out.push(audio);
            }
            if out.is_empty() {
                return Err(vendor_err("Polly returned no audio"));
            }
            Ok(out)
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = (text, sink);
            Err(SpeechError::Vendor {
                vendor: "aws".into(),
                message: "live AWS TTS requires `--features live` on vendor-aws".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
fn vendor_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "aws".into(),
        message: message.into(),
    }
}

/// 16 kHz mono s16le (Polly `pcm` only allows 8000 and 16000 Hz) as the agent's 48 kHz stereo.
/// Each sample is repeated three times; no resampler exists in this workspace for TTS.
#[cfg(feature = "live")]
fn agent_chunk(mono_16k: &[u8]) -> TtsAudioChunk {
    let mut stretched = Vec::with_capacity(mono_16k.len() * 3);
    for sample in mono_16k.as_chunks::<2>().0 {
        for _ in 0..3 {
            stretched.extend_from_slice(sample);
        }
    }
    TtsAudioChunk {
        duration_ms: duration_ms_from_mono_s16le(mono_16k.len(), POLLY_PCM_RATE),
        pcm: mono_s16le_to_stereo(&stretched),
    }
}

#[cfg(feature = "live")]
fn voice_id_from_name(name: &str) -> Result<aws_sdk_polly::types::VoiceId, String> {
    use aws_sdk_polly::types::VoiceId;
    match name {
        "Joanna" => Ok(VoiceId::Joanna),
        "Matthew" => Ok(VoiceId::Matthew),
        "Amy" => Ok(VoiceId::Amy),
        "Brian" => Ok(VoiceId::Brian),
        "Ruth" => Ok(VoiceId::Ruth),
        "Stephen" => Ok(VoiceId::Stephen),
        other => Err(format!("unsupported Polly voice id `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn rejects_undocumented_voice() {
        assert!(matches!(
            AwsTts::new(&TtsConfig {
                provider: TtsVendor::Aws,
                model: None,
                model_path: None,
                voice: Some("Siri".into()),
                api_key: None,
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
