use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use node_webrtc_rust_speech::config::{TtsConfig, VoiceSessionContext};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};
use node_webrtc_rust_speech_proto::v1::speech_client::SpeechClient;
use node_webrtc_rust_speech_proto::v1::{ModelRef, SessionContext, SynthesizeRequest};
use tonic::metadata::MetadataValue;
use tonic::Request;

use crate::channel::{
    resolve_speech_token, resolve_tts_endpoint, tts_channel, MAX_GRPC_MESSAGE_BYTES,
};
use crate::stt::{auth_metadata, session_context_proto};

pub struct ClusterSherpaTts {
    cfg: TtsConfig,
    endpoint: String,
    token: Option<String>,
    session_ctx: Arc<tokio::sync::Mutex<Option<VoiceSessionContext>>>,
}

impl ClusterSherpaTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let endpoint = resolve_tts_endpoint(config)?;
        let token = resolve_speech_token(&config.api_key);
        Ok(Self {
            cfg: config.clone(),
            endpoint,
            token,
            session_ctx: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }
}

#[async_trait]
impl TtsProvider for ClusterSherpaTts {
    fn vendor_name(&self) -> &'static str {
        "cluster-sherpa"
    }

    fn bind_session_context(&self, ctx: &VoiceSessionContext) {
        if let Ok(mut guard) = self.session_ctx.try_lock() {
            *guard = Some(ctx.clone());
        }
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let channel = tts_channel(&self.endpoint).await?;
        let mut client =
            SpeechClient::new(channel).max_decoding_message_size(MAX_GRPC_MESSAGE_BYTES);
        let model_path = self
            .cfg
            .model_path
            .clone()
            .unwrap_or_else(|| "/models/sherpa/tts/default".to_string());
        let speaker_id = self.cfg.voice.clone().unwrap_or_default();
        let speed = self.cfg.model.clone().unwrap_or_default();
        let ctx = self
            .session_ctx
            .lock()
            .await
            .as_ref()
            .map(session_context_proto)
            .unwrap_or_default();
        let req = SynthesizeRequest {
            model: Some(ModelRef {
                model_path,
                catalog_id: String::new(),
            }),
            text: text.to_string(),
            speaker_id,
            speed,
            ctx: Some(ctx),
        };
        let mut request = Request::new(req);
        if let Ok(auth) = auth_metadata(&self.token) {
            request.metadata_mut().insert("authorization", auth);
        }
        let mut stream = client
            .synthesize(request)
            .await
            .map_err(|e| SpeechError::Vendor {
                vendor: "cluster-sherpa".into(),
                message: e.message().to_string(),
            })?
            .into_inner();

        let cancel = sink.as_ref().map(|s| Arc::clone(&s.cancel));
        let mut collected = Vec::new();
        while let Some(msg) = stream.message().await.map_err(|e| SpeechError::Vendor {
            vendor: "cluster-sherpa".into(),
            message: e.message().to_string(),
        })? {
            if cancel
                .as_ref()
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(false)
            {
                break;
            }
            let chunk = TtsAudioChunk {
                pcm: msg.pcm_s16le,
                duration_ms: msg.duration_ms,
            };
            if let Some(s) = sink.as_ref() {
                if !s.send(chunk.clone()) {
                    break;
                }
            }
            collected.push(chunk);
            if msg.last {
                break;
            }
        }
        Ok(collected)
    }
}
