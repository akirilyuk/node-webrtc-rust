use std::sync::Arc;

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};
use sherpa_onnx::GenerationConfig;
use tokio::sync::Mutex;

use crate::audio::f32_mono_to_stereo_48k_s16le;
use crate::pool::{SharedTtsEngine, SherpaModelPool};
pub struct SherpaTts {
    config: TtsConfig,
    pool: Arc<crate::pool::SherpaModelPool>,
    shared: Arc<Mutex<Option<Arc<SharedTtsEngine>>>>,
    speaker_id: i32,
    speed: f32,
}

impl SherpaTts {
    pub fn new(config: &TtsConfig) -> Self {
        Self {
            config: config.clone(),
            pool: SherpaModelPool::global(),
            shared: Arc::new(Mutex::new(None)),
            speaker_id: parse_speaker_id(config),
            speed: parse_speed(config),
        }
    }

    async fn ensure_engine(&self) -> SpeechResult<Arc<SharedTtsEngine>> {
        let mut guard = self.shared.lock().await;
        if let Some(shared) = guard.as_ref() {
            return Ok(Arc::clone(shared));
        }

        let config = self.config.clone();
        let pool = Arc::clone(&self.pool);
        let shared = tokio::task::spawn_blocking(move || pool.get_or_create_tts(&config))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??;
        shared.session_started();
        *guard = Some(Arc::clone(&shared));
        Ok(shared)
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

        let shared = self.ensure_engine().await?;
        let input = trimmed.to_string();
        let speaker_id = self.speaker_id;
        let speed = self.speed;
        let tts_semaphore = shared.tts_semaphore();
        let _permit = tts_semaphore
            .acquire()
            .await
            .map_err(|_| SpeechError::Internal("sherpa TTS semaphore closed".into()))?;

        let chunk = tokio::task::spawn_blocking(move || -> SpeechResult<TtsAudioChunk> {
            let _synthesis_guard = shared
                .synthesis_mutex()
                .lock()
                .map_err(|_| SpeechError::Internal("sherpa TTS synthesis lock poisoned".into()))?;

            let gen_config = GenerationConfig {
                sid: speaker_id,
                speed,
                ..Default::default()
            };

            let tts = shared
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

        Ok(vec![chunk])
    }
}

impl Drop for SherpaTts {
    fn drop(&mut self) {
        if let Ok(guard) = self.shared.try_lock() {
            if let Some(shared) = guard.as_ref() {
                shared.session_ended();
            }
        }
    }
}
