//! Azure TTS — documented REST SSML synthesis (full body PCM).

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::factory::speech_key_from;
use crate::matrix::{default_tts_voice, tts_rest_url, validate_tts_voice};

const AZURE_TTS_PCM_RATE: u32 = 16_000;

pub struct AzureTts {
    speech_key: Option<String>,
    region: String,
    voice: String,
}

impl AzureTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let voice = config
            .voice
            .clone()
            .unwrap_or_else(|| default_tts_voice().to_string());
        validate_tts_voice(&voice).map_err(SpeechError::Config)?;
        let region = resolve_region(config)?;
        Ok(Self {
            speech_key: config
                .api_key
                .clone()
                .or_else(|| {
                    ["AZURE_SPEECH_KEY", "SPEECH_KEY"]
                        .into_iter()
                        .find_map(|env| std::env::var(env).ok())
                }),
            region,
            voice,
        })
    }
}

fn resolve_region(config: &TtsConfig) -> SpeechResult<String> {
    if let Some(endpoint) = config.endpoint.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
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

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_full_body(text).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let chunks = self.synthesize_full_body(text).await?;
        if let Some(s) = sink.as_ref() {
            for chunk in &chunks {
                let _ = s.send(chunk.clone());
            }
        }
        Ok(chunks)
    }
}

impl AzureTts {
    async fn synthesize_full_body(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let key = speech_key_from(&self.speech_key)?;
            let url = tts_rest_url(&self.region);
            let ssml = format!(
                "<speak version='1.0' xml:lang='en-US'><voice name='{}'>{}</voice></speak>",
                self.voice,
                xml_escape(text)
            );
            let response = reqwest::Client::new()
                .post(url)
                .header("Ocp-Apim-Subscription-Key", key)
                .header("Content-Type", "application/ssml+xml")
                .header("X-Microsoft-OutputFormat", "riff-16khz-16bit-mono-pcm")
                .body(ssml)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "azure".into(),
                    message: err.to_string(),
                })?;
            if !response.status().is_success() {
                let status = response.status();
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "azure".into(),
                    message,
                });
            }
            let wav = response.bytes().await.map_err(|err| SpeechError::Vendor {
                vendor: "azure".into(),
                message: err.to_string(),
            })?;
            let mono = wav_pcm_payload(&wav).map_err(SpeechError::Config)?;
            let duration_ms = duration_ms_from_mono_s16le(mono.len(), AZURE_TTS_PCM_RATE);
            let pcm = mono_s16le_to_stereo(&mono);
            Ok(vec![TtsAudioChunk { pcm, duration_ms }])
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = text;
            Err(SpeechError::Vendor {
                vendor: "azure".into(),
                message: "live Azure TTS requires `--features live` on vendor-azure".into(),
            })
        }
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn wav_pcm_payload(wav: &[u8]) -> Result<Vec<u8>, String> {
    if wav.len() < 44 || !wav.starts_with(b"RIFF") {
        return Err("Azure TTS response is not a WAV file".into());
    }
    Ok(wav[44..].to_vec())
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
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
