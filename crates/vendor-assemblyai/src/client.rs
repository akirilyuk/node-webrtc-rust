//! Minimal AssemblyAI realtime WS client stub.

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

pub struct AssemblyAiClient {
    api_key: Option<String>,
}

impl AssemblyAiClient {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }

    pub async fn connect(&self) -> SpeechResult<()> {
        if self.api_key.is_none() && std::env::var("ASSEMBLYAI_API_KEY").is_err() {
            return Err(SpeechError::Config("missing ASSEMBLYAI_API_KEY".into()));
        }
        Err(SpeechError::Vendor {
            vendor: "assemblyai".into(),
            message: "AssemblyAI live WS wiring pending".into(),
        })
    }
}
