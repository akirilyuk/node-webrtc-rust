use std::sync::Arc;

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};
use sherpa_onnx::GenerationConfig;
use tokio::sync::Mutex;

use crate::audio::f32_mono_to_stereo_48k_s16le;
use crate::pool::{SherpaModelPool, TtsEnginePool};

fn voice_debug_enabled() -> bool {
    matches!(
        std::env::var("VOICE_DEBUG").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn voice_debug(message: impl AsRef<str>) {
    if voice_debug_enabled() {
        eprintln!("[voice-debug] {}", message.as_ref());
    }
}

pub struct SherpaTts {
    config: TtsConfig,
    pool: Arc<crate::pool::SherpaModelPool>,
    engine_pool: Arc<Mutex<Option<Arc<TtsEnginePool>>>>,
    speaker_id: i32,
    speed: f32,
}

impl SherpaTts {
    pub fn new(config: &TtsConfig) -> Self {
        Self {
            config: config.clone(),
            pool: SherpaModelPool::global(),
            engine_pool: Arc::new(Mutex::new(None)),
            speaker_id: parse_speaker_id(config),
            speed: parse_speed(config),
        }
    }

    async fn ensure_engine_pool(&self) -> SpeechResult<Arc<TtsEnginePool>> {
        let mut guard = self.engine_pool.lock().await;
        if let Some(pool) = guard.as_ref() {
            return Ok(Arc::clone(pool));
        }

        let config = self.config.clone();
        let pool = Arc::clone(&self.pool);
        let engine_pool = tokio::task::spawn_blocking(move || pool.get_or_create_tts(&config))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??;
        *guard = Some(Arc::clone(&engine_pool));
        Ok(engine_pool)
    }
}

pub(crate) fn parse_speaker_id(config: &TtsConfig) -> i32 {
    config
        .voice
        .as_deref()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0)
}

pub(crate) fn parse_speed(config: &TtsConfig) -> f32 {
    config
        .model
        .as_deref()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .or_else(|| {
            std::env::var("SHERPA_TTS_SPEED")
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(1.0)
        .clamp(0.5, 2.0)
}

#[async_trait]
impl TtsProvider for SherpaTts {
    fn vendor_name(&self) -> &'static str {
        "local-sherpa"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let engine_pool = self.ensure_engine_pool().await?;
        let shared = engine_pool.acquire();
        shared.session_started();
        let input = trimmed.to_string();
        let speaker_id = self.speaker_id;
        let speed = self.speed;
        let text_len = trimmed.len();
        let tts_semaphore = self.pool.tts_semaphore();
        let _permit = tts_semaphore
            .acquire()
            .await
            .map_err(|_| SpeechError::Internal("sherpa TTS semaphore closed".into()))?;

        voice_debug(format!("tts synthesis start text_len={text_len}"));
        let wall_start = std::time::Instant::now();

        let shared_for_blocking = Arc::clone(&shared);
        let chunk = tokio::task::spawn_blocking(move || -> SpeechResult<TtsAudioChunk> {
            let gen_config = GenerationConfig {
                sid: speaker_id,
                speed,
                ..Default::default()
            };

            let tts = shared_for_blocking
                .tts
                .lock()
                .map_err(|_| SpeechError::Internal("sherpa TTS engine lock poisoned".into()))?;

            let audio = tts
                .generate_with_config(&input, &gen_config, None::<fn(&[f32], f32) -> bool>)
                .ok_or_else(|| SpeechError::Vendor {
                    vendor: "local-sherpa".into(),
                    message: "OfflineTts generation returned no audio".into(),
                })?;

            let src_rate = audio.sample_rate().max(1) as u32;
            let (pcm, duration_ms) = f32_mono_to_stereo_48k_s16le(audio.samples(), src_rate);

            Ok(TtsAudioChunk { pcm, duration_ms })
        })
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))??;

        voice_debug(format!(
            "tts synthesis done wall_ms={} audio_duration_ms={}",
            wall_start.elapsed().as_millis(),
            chunk.duration_ms
        ));
        shared.session_ended();
        Ok(vec![chunk])
    }
}
