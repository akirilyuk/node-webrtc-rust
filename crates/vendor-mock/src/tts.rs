use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

pub struct MockTts {
    voice: String,
}

impl MockTts {
    pub fn new(config: &TtsConfig) -> Self {
        Self {
            voice: config.voice.clone().unwrap_or_else(|| "mock".into()),
        }
    }
}

fn synthesize_sine_pcm(duration_ms: u32, frequency_hz: f32) -> Bytes {
    let sample_rate = 48_000_u32;
    let samples = (sample_rate * duration_ms / 1000) as usize;
    let mut pcm = Vec::with_capacity(samples * 4);
    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (t * frequency_hz * 2.0 * std::f32::consts::PI).sin() * 0.2;
        let i16_sample = (sample * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&i16_sample.to_le_bytes());
        pcm.extend_from_slice(&i16_sample.to_le_bytes());
    }
    Bytes::from(pcm)
}

#[async_trait]
impl TtsProvider for MockTts {
    fn vendor_name(&self) -> &'static str {
        "mock"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let duration_ms = (text.len() as u32 * 50).clamp(100, 5000);
        let freq = 440.0 + (self.voice.len() as f32 * 10.0);
        Ok(vec![TtsAudioChunk {
            pcm: synthesize_sine_pcm(duration_ms, freq),
            duration_ms,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_tts_returns_pcm() {
        let tts = MockTts::new(&TtsConfig {
            provider: node_webrtc_rust_speech::config::TtsVendor::Mock,
            model: None,
            voice: Some("test".into()),
            api_key: None,
        });
        let chunks = tts.synthesize("hello").await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].pcm.len() > 0);
    }
}
