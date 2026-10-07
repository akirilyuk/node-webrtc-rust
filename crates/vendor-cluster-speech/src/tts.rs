use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use node_webrtc_rust_speech::config::{TtsConfig, VoiceSessionContext};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{
    TtsAudioChunk, TtsOpenWait, TtsProgressiveSink, TtsProvider, TtsSynthesis,
};
use node_webrtc_rust_speech_proto::v1::speech_client::SpeechClient;
use node_webrtc_rust_speech_proto::v1::{ModelRef, SessionContext, SynthesizeRequest};
use tonic::metadata::MetadataValue;
use tonic::{Code, Request};

use crate::channel::{
    resolve_speech_token, resolve_tts_endpoint, tts_channel, MAX_GRPC_MESSAGE_BYTES,
};
use crate::metrics::{inc_tts_open_retries, record_tts_open_wait_ms};
use crate::stt::{auth_metadata, session_context_proto};

/// Longest a synthesis start waits for a speech pod that refuses it with `UNAVAILABLE` (models
/// still loading, pod restarting) or `RESOURCE_EXHAUSTED` (pod full) before the synthesis fails.
/// Override with `SPEECH_TTS_OPEN_WAIT_MAX_MS`; `0` disables the wait.
const TTS_OPEN_WAIT_MAX_MS: u64 = 30_000;
/// Pause between attempts at a refused synthesis start.
const TTS_OPEN_RETRY_MS: u64 = 200;

fn open_wait_max_ms_from_env() -> u64 {
    static VALUE: OnceLock<u64> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("SPEECH_TTS_OPEN_WAIT_MAX_MS")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(TTS_OPEN_WAIT_MAX_MS)
    })
}

fn is_open_refusal(code: Code) -> bool {
    matches!(code, Code::Unavailable | Code::ResourceExhausted)
}

fn vendor_error(status: &tonic::Status) -> SpeechError {
    SpeechError::Vendor {
        vendor: "cluster-sherpa".into(),
        message: status.message().to_string(),
    }
}

pub struct ClusterSherpaTts {
    cfg: TtsConfig,
    endpoint: String,
    token: Option<String>,
    session_ctx: Arc<tokio::sync::Mutex<Option<VoiceSessionContext>>>,
    open_wait_max_ms: u64,
    last_open_wait: std::sync::Mutex<Option<TtsOpenWait>>,
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
            open_wait_max_ms: open_wait_max_ms_from_env(),
            last_open_wait: std::sync::Mutex::new(None),
        })
    }

    /// Override how long a refused synthesis start waits (default `SPEECH_TTS_OPEN_WAIT_MAX_MS`,
    /// 30 s; `0` = fail at the first refusal).
    pub fn with_open_wait_max_ms(mut self, ms: u64) -> Self {
        self.open_wait_max_ms = ms;
        self
    }

    /// Record the finished wait for the metrics and for [`TtsProvider::take_open_wait`].
    fn finish_open_wait(&self, wait_ms: u64, attempts: u32, reason: &str) {
        record_tts_open_wait_ms(wait_ms);
        *self.last_open_wait.lock().unwrap() = Some(TtsOpenWait {
            wait_ms: u32::try_from(wait_ms).unwrap_or(u32::MAX),
            attempts,
            reason: reason.to_string(),
        });
    }

    /// Run one `Synthesize` stream. `complete` is true only when the server sent its
    /// terminal `last` message and the sink was not cancelled; a stream that ends
    /// early or is cut by the sink yields partial audio.
    async fn stream_synthesis(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<TtsSynthesis> {
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
        let cancel = sink.as_ref().map(|s| Arc::clone(&s.cancel));
        let is_cancelled = || {
            cancel
                .as_ref()
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(false)
        };

        // Open the stream. A refusal before any audio (`UNAVAILABLE` / `RESOURCE_EXHAUSTED`, on
        // the call or as the first stream message) is retried until the open-wait cap.
        *self.last_open_wait.lock().unwrap() = None;
        let wait_started = Instant::now();
        let mut attempts: u32 = 0;
        let (mut stream, first) = loop {
            let mut request = Request::new(req.clone());
            if let Ok(auth) = auth_metadata(&self.token) {
                request.metadata_mut().insert("authorization", auth);
            }
            let refusal = match client.synthesize(request).await {
                Ok(resp) => {
                    let mut stream = resp.into_inner();
                    match stream.message().await {
                        Ok(first) => break (stream, first),
                        Err(status) if is_open_refusal(status.code()) => status,
                        Err(status) => return Err(vendor_error(&status)),
                    }
                }
                Err(status) if is_open_refusal(status.code()) => status,
                Err(status) => return Err(vendor_error(&status)),
            };
            let reason = format!("{:?} {}", refusal.code(), refusal.message());
            let waited_ms = wait_started.elapsed().as_millis() as u64;
            if is_cancelled() {
                return Ok(TtsSynthesis {
                    chunks: Vec::new(),
                    complete: false,
                });
            }
            if waited_ms >= self.open_wait_max_ms {
                if attempts > 0 {
                    self.finish_open_wait(waited_ms, attempts, &reason);
                }
                eprintln!(
                    "[voice] tts start gave up after {waited_ms}ms: {:?} {}",
                    refusal.code(),
                    refusal.message()
                );
                return Err(vendor_error(&refusal));
            }
            attempts += 1;
            inc_tts_open_retries(1);
            let remaining = self.open_wait_max_ms - waited_ms;
            tokio::time::sleep(Duration::from_millis(TTS_OPEN_RETRY_MS.min(remaining))).await;
            // Remember the latest refusal for the log line / event once the start succeeds.
            *self.last_open_wait.lock().unwrap() = Some(TtsOpenWait {
                wait_ms: 0,
                attempts,
                reason,
            });
        };
        if attempts > 0 {
            let waited_ms = wait_started.elapsed().as_millis() as u64;
            let reason = self
                .last_open_wait
                .lock()
                .unwrap()
                .as_ref()
                .map(|w| w.reason.clone())
                .unwrap_or_default();
            self.finish_open_wait(waited_ms, attempts, &reason);
            eprintln!(
                "[voice] tts start waited {waited_ms}ms ({attempts} attempts, last: {reason})"
            );
        }

        let mut collected = Vec::new();
        let mut saw_last = false;
        let mut pending = first;
        loop {
            let msg = match pending.take() {
                Some(m) => m,
                None => match stream.message().await.map_err(|e| vendor_error(&e))? {
                    Some(m) => m,
                    None => break,
                },
            };
            if is_cancelled() {
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
                saw_last = true;
                break;
            }
        }
        let cancelled = is_cancelled();
        Ok(TtsSynthesis {
            chunks: collected,
            complete: saw_last && !cancelled,
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

    fn take_open_wait(&self) -> Option<TtsOpenWait> {
        self.last_open_wait.lock().unwrap().take()
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        Ok(self.stream_synthesis(text, sink).await?.chunks)
    }

    async fn synthesize_progressive_with_status(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<TtsSynthesis> {
        self.stream_synthesis(text, sink).await
    }
}
