//! Minimal Cartesia REST client (live calls require CARTESIA_API_KEY).

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

pub struct CartesiaClient {
    api_key: Option<String>,
    base_url: String,
}

impl CartesiaClient {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            base_url: "https://api.cartesia.ai".to_string(),
        }
    }

    pub async fn synthesize_text(&self, text: &str, voice: &str) -> SpeechResult<Vec<u8>> {
        let _ = (text, voice, &self.base_url);
        if self.api_key.is_none() && std::env::var("CARTESIA_API_KEY").is_err() {
            return Err(SpeechError::Config("missing CARTESIA_API_KEY".into()));
        }
        Err(SpeechError::Vendor {
            vendor: "cartesia".into(),
            message: "Cartesia live TTS wiring pending".into(),
        })
    }
}
