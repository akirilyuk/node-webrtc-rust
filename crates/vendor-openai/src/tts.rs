use async_trait::async_trait;
#[cfg(feature = "live")]
use async_openai::types::{
    CreateSpeechRequestArgs, SpeechModel, SpeechResponseFormat, Voice,
};
#[cfg(feature = "live")]
use async_openai::Client;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

use crate::factory::api_key_from;

/// OpenAI TTS PCM output sample rate (16-bit mono LE).
const OPENAI_TTS_PCM_SAMPLE_RATE: u32 = 24_000;

pub struct OpenAiTts {
    api_key: Option<String>,
    model: String,
    voice: String,
}

impl OpenAiTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config.api_key.clone().or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "tts-1".to_string()),
            voice: config
                .voice
                .clone()
                .unwrap_or_else(|| "alloy".to_string()),
        })
    }
}

#[async_trait]
impl TtsProvider for OpenAiTts {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
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

            let client =
                Client::with_config(async_openai::config::OpenAIConfig::new().with_api_key(api_key));
            let response = client.audio().speech(request).await.map_err(|err| {
                SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: err.to_string(),
                }
            })?;

            let mono_24k = response.bytes;
            let duration_ms =
                duration_ms_from_mono_s16le(mono_24k.len(), OPENAI_TTS_PCM_SAMPLE_RATE);
            let pcm = mono_24k_s16le_to_stereo_48k(mono_24k.as_ref());
            return Ok(vec![TtsAudioChunk { pcm, duration_ms }]);
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = api_key_from(&self.api_key, "OPENAI_API_KEY");
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI TTS requires `--features live` on vendor-openai".into(),
            })
        }
    }
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

    #[cfg(feature = "live")]
    #[test]
    fn parse_voice_and_model() {
        assert!(matches!(parse_voice("alloy"), Voice::Alloy));
        assert!(matches!(parse_speech_model("tts-1-hd"), SpeechModel::Tts1Hd));
    }

    #[test]
    fn factory_defaults() {
        let tts = OpenAiTts::new(&TtsConfig {
            provider: TtsVendor::Openai,
            model: None,
            model_path: None,
            voice: None,
            api_key: Some("test".into()),
        })
        .unwrap();
        assert_eq!(tts.model, "tts-1");
        assert_eq!(tts.voice, "alloy");
    }
}
