//! AWS Polly `SynthesizeSpeech` PCM (documented REST/SDK; no generative stream in this crate).

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::matrix::{default_tts_voice, validate_tts_voice};

const POLLY_PCM_RATE: u32 = 16_000;

pub struct AwsTts {
    voice: String,
}

impl AwsTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let voice = config
            .voice
            .clone()
            .unwrap_or_else(|| default_tts_voice().to_string());
        validate_tts_voice(&voice).map_err(SpeechError::Config)?;
        Ok(Self { voice })
    }
}

#[async_trait]
impl TtsProvider for AwsTts {
    fn vendor_name(&self) -> &'static str {
        "aws"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_pcm(text).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let chunks = self.synthesize_pcm(text).await?;
        if let Some(s) = sink.as_ref() {
            for chunk in &chunks {
                let _ = s.send(chunk.clone());
            }
        }
        Ok(chunks)
    }
}

impl AwsTts {
    async fn synthesize_pcm(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            use futures_util::StreamExt;
            let voice_id = voice_id_from_name(&self.voice).map_err(SpeechError::Config)?;
            let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await;
            let client = aws_sdk_polly::Client::new(&config);
            let output = client
                .synthesize_speech()
                .text(text)
                .voice_id(voice_id)
                .output_format(aws_sdk_polly::types::OutputFormat::Pcm)
                .sample_rate("16000")
                .engine(aws_sdk_polly::types::Engine::Neural)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "aws".into(),
                    message: err.to_string(),
                })?;
            let mono = output
                .audio_stream
                .collect()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "aws".into(),
                    message: err.to_string(),
                })?
                .into_bytes()
                .to_vec();
            let duration_ms = duration_ms_from_mono_s16le(mono.len(), POLLY_PCM_RATE);
            let pcm = mono_s16le_to_stereo(&mono);
            Ok(vec![TtsAudioChunk { pcm, duration_ms }])
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = text;
            Err(SpeechError::Vendor {
                vendor: "aws".into(),
                message: "live AWS TTS requires `--features live` on vendor-aws".into(),
            })
        }
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
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
