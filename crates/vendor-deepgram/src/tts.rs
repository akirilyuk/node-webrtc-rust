use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::tts_matrix::{
    default_speak_model, flush_json, speak_json, tts_progressive_transport, v1_speak_rest_url,
    v2_speak_rest_url, v1_speak_websocket_url, v2_speak_websocket_url, TtsProgressiveTransport,
};

pub struct DeepgramTts {
    api_key: Option<String>,
    model: String,
}

impl DeepgramTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_speak_model().to_string());
        crate::tts_matrix::validate_speak_model(&model).map_err(SpeechError::Config)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok()),
            model,
        })
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing DEEPGRAM_API_KEY".into()))
    }
}

#[async_trait]
impl TtsProvider for DeepgramTts {
    fn vendor_name(&self) -> &'static str {
        "deepgram"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_rest(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        match tts_progressive_transport(&self.model).map_err(SpeechError::Config)? {
            TtsProgressiveTransport::WebSocketV1 | TtsProgressiveTransport::WebSocketV2 => {
                #[cfg(feature = "live")]
                {
                    return self.synthesize_websocket(text, sink).await;
                }
                #[cfg(not(feature = "live"))]
                {
                    let chunks = self.synthesize_rest(text, sink.as_ref()).await?;
                    emit_chunks(sink.as_ref(), &chunks);
                    Ok(chunks)
                }
            }
            TtsProgressiveTransport::RestFullBody => {
                let chunks = self.synthesize_rest(text, sink.as_ref()).await?;
                emit_chunks(sink.as_ref(), &chunks);
                Ok(chunks)
            }
        }
    }
}

impl DeepgramTts {
    async fn synthesize_rest(
        &self,
        text: &str,
        _sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let url = if self.model.starts_with("flux-") {
                v2_speak_rest_url(&self.model).map_err(SpeechError::Config)?
            } else {
                v1_speak_rest_url(&self.model).map_err(SpeechError::Config)?
            };
            let body = serde_json::json!({ "text": text });

            let response = reqwest::Client::new()
                .post(url)
                .header("Authorization", format!("Token {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "deepgram".into(),
                    message: err.to_string(),
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "deepgram".into(),
                    message,
                });
            }

            let mono = response.bytes().await.map_err(|err| SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: err.to_string(),
            })?;
            let chunk = pcm_chunk_from_mono(mono.as_ref());
            if let Some(s) = _sink {
                let _ = s.send(chunk.clone());
            }
            return Ok(vec![chunk]);
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, self.api_key()?);
            Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "live Deepgram TTS requires `--features live` on vendor-deepgram".into(),
            })
        }
    }

    #[cfg(feature = "live")]
    async fn synthesize_websocket(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{
            connect_async,
            tungstenite::{client::IntoClientRequest, Message},
        };

        let api_key = self.api_key()?;
        let url = if self.model.starts_with("flux-") {
            v2_speak_websocket_url(&self.model).map_err(SpeechError::Config)?
        } else {
            v1_speak_websocket_url(&self.model).map_err(SpeechError::Config)?
        };

        let mut request = url
            .into_client_request()
            .map_err(|err| SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: err.to_string(),
            })?;
        request.headers_mut().insert(
            "Authorization",
            format!("Token {api_key}")
                .parse()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "deepgram".into(),
                    message: format!("invalid auth header: {err}"),
                })?,
        );

        let (ws, _) = connect_async(request)
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: err.to_string(),
            })?;
        let (mut ws_tx, mut ws_rx) = ws.split();

        ws_tx
            .send(Message::Text(speak_json(text).into()))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: err.to_string(),
            })?;
        ws_tx
            .send(Message::Text(flush_json().into()))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: err.to_string(),
            })?;

        let mut mono = Vec::new();
        let mut turn_done = false;
        while let Some(msg) = ws_rx.next().await {
            let Ok(msg) = msg else { break };
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                break;
            }
            match msg {
                Message::Binary(chunk) => {
                    mono.extend_from_slice(&chunk);
                    if let Some(s) = sink.as_ref() {
                        let audio = pcm_chunk_from_mono(&chunk);
                        let _ = s.send(audio);
                    }
                }
                Message::Text(raw) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                        let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if msg_type == "SpeechMetadata"
                            || msg_type == "Flushed"
                            || msg_type == "Cleared"
                        {
                            turn_done = true;
                        }
                        if msg_type == "Error" {
                            let description = value
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Deepgram speak error");
                            return Err(SpeechError::Vendor {
                                vendor: "deepgram".into(),
                                message: description.to_string(),
                            });
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
            if turn_done {
                break;
            }
        }

        let _ = ws_tx.send(Message::Close(None)).await;
        if mono.is_empty() {
            return Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "Deepgram speak websocket returned no audio".into(),
            });
        }
        Ok(vec![pcm_chunk_from_mono(&mono)])
    }
}

fn pcm_chunk_from_mono(mono: &[u8]) -> TtsAudioChunk {
    let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
    let pcm = mono_s16le_to_stereo(mono);
    TtsAudioChunk { pcm, duration_ms }
}

fn emit_chunks(sink: Option<&TtsProgressiveSink>, chunks: &[TtsAudioChunk]) {
    if let Some(s) = sink {
        for chunk in chunks {
            let _ = s.send(chunk.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn rejects_flux_model_on_aura_only_config_mismatch() {
        assert!(matches!(
            DeepgramTts::new(&TtsConfig {
                provider: TtsVendor::Deepgram,
                model: Some("not-a-model".into()),
                model_path: None,
                voice: None,
                api_key: Some("k".into()),
                endpoint: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }

    #[test]
    fn accepts_aura_and_flux_models() {
        for model in ["aura-asteria-en", "flux-haley-en"] {
            assert!(DeepgramTts::new(&TtsConfig {
                provider: TtsVendor::Deepgram,
                model: Some(model.into()),
                model_path: None,
                voice: None,
                api_key: Some("k".into()),
                endpoint: None,
            })
            .is_ok());
        }
    }
}
