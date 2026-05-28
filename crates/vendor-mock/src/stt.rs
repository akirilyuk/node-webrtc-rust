use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

const FINAL_THRESHOLD_BYTES: usize = 3200;

pub struct MockStt {
    language: String,
    buffered: Vec<u8>,
    running: bool,
    emitted_partial: bool,
}

impl MockStt {
    pub fn new(config: &SttConfig) -> Self {
        Self {
            language: config.language.clone().unwrap_or_else(|| "en".into()),
            buffered: Vec::new(),
            running: false,
            emitted_partial: false,
        }
    }
}

#[async_trait]
impl SttProvider for MockStt {
    fn vendor_name(&self) -> &'static str {
        "mock"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.running = true;
        self.buffered.clear();
        self.emitted_partial = false;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.running = false;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if self.running {
            self.buffered.extend_from_slice(pcm.as_ref());
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        if !self.running {
            return Ok(None);
        }
        if !self.emitted_partial && self.buffered.len() >= FINAL_THRESHOLD_BYTES / 2 {
            self.emitted_partial = true;
            return Ok(Some(SttTranscript::Partial(format!(
                "[mock-{lang}] listening…",
                lang = self.language
            ))));
        }
        if self.buffered.len() >= FINAL_THRESHOLD_BYTES {
            self.buffered.clear();
            self.emitted_partial = false;
            return Ok(Some(SttTranscript::Final(format!(
                "[mock-{lang}] hello world",
                lang = self.language
            ))));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_stt_emits_final_after_threshold() {
        let mut stt = MockStt::new(&SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::Mock,
            model: None,
            language: Some("en".into()),
            api_key: None,
        });
        stt.start().await.unwrap();
        stt.push_audio(Bytes::from(vec![1_u8; FINAL_THRESHOLD_BYTES]))
            .await
            .unwrap();
        let final_text = stt.poll_transcript().await.unwrap();
        assert!(matches!(final_text, Some(SttTranscript::Final(_))));
    }
}
