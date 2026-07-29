use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use node_webrtc_rust_speech::config::{TtsConfig, VoiceSessionContext};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel::{self, SherpaTtsMetricAttrs};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};
use sherpa_onnx::GenerationConfig;
use tokio::sync::Mutex;

use crate::audio::f32_mono_to_stereo_48k_s16le;
use crate::phrase_cache::{
    build_cache_key, build_metric_attrs, lookup, normalize_phrase_text, phrase_cache_enabled, store,
};
use crate::pool::{SherpaModelPool, TtsEnginePool};
use crate::tts_model_paths::resolve_tts_model_dir_path;

static TTS_GENERATE_COUNT: AtomicUsize = AtomicUsize::new(0);

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
    project_id: Arc<StdMutex<String>>,
    resolved_model_dir: Arc<Mutex<Option<String>>>,
}

impl SherpaTts {
    pub fn new(config: &TtsConfig) -> Self {
        Self {
            config: config.clone(),
            pool: SherpaModelPool::global(),
            engine_pool: Arc::new(Mutex::new(None)),
            speaker_id: parse_speaker_id(config),
            speed: parse_speed(config),
            project_id: Arc::new(StdMutex::new(String::new())),
            resolved_model_dir: Arc::new(Mutex::new(None)),
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

    async fn resolved_model_dir(&self) -> SpeechResult<String> {
        let mut guard = self.resolved_model_dir.lock().await;
        if let Some(path) = guard.as_ref() {
            return Ok(path.clone());
        }
        let config = self.config.clone();
        let model_dir = tokio::task::spawn_blocking(move || resolve_tts_model_dir_path(&config))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??
            .display()
            .to_string();
        *guard = Some(model_dir.clone());
        Ok(model_dir)
    }

    fn session_project_id(&self) -> String {
        let from_session = self
            .project_id
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let trimmed = from_session.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        // Runner pods set PROJECT_ID; covers TTS before/without bind_session_context.
        std::env::var("PROJECT_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_default()
    }

    fn voice_label(&self) -> String {
        self.config
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("0")
            .to_string()
    }

    fn language_label(&self) -> String {
        // Piper/Sherpa local TTS has no language field on TtsConfig; default en so
        // OTel `tts.language` is never dropped (empty attrs are omitted in Prometheus).
        std::env::var("SHERPA_TTS_LANGUAGE")
            .ok()
            .or_else(|| std::env::var("SHERPA_LANGUAGE").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "en".to_string())
    }

    async fn synthesize_miss(
        &self,
        normalized: &str,
        attrs: &SherpaTtsMetricAttrs,
    ) -> SpeechResult<TtsAudioChunk> {
        let engine_pool = self.ensure_engine_pool().await?;
        let shared = engine_pool.acquire();
        let input = normalized.to_string();
        let speaker_id = self.speaker_id;
        let speed = self.speed;
        let text_len = normalized.len();
        let tts_semaphore = self.pool.tts_semaphore();

        let queue_wait_start = std::time::Instant::now();
        let _permit = tts_semaphore
            .acquire()
            .await
            .map_err(|_| SpeechError::Internal("sherpa TTS semaphore closed".into()))?;
        let queue_wait_ms = queue_wait_start.elapsed().as_secs_f64() * 1000.0;
        otel::record_sherpa_pool_wait_ms(queue_wait_ms);
        otel::record_sherpa_tts_queue_wait_ms(queue_wait_ms, attrs);

        voice_debug(format!("tts synthesis start text_len={text_len}"));
        let wall_start = std::time::Instant::now();

        let shared_for_blocking = Arc::clone(&shared);
        let chunk = tokio::task::spawn_blocking(move || -> SpeechResult<TtsAudioChunk> {
            let _active = shared_for_blocking.track_session();
            let gen_config = GenerationConfig {
                sid: speaker_id,
                speed,
                ..Default::default()
            };

            let tts = shared_for_blocking
                .tts
                .lock()
                .map_err(|_| SpeechError::Internal("sherpa TTS engine lock poisoned".into()))?;

            TTS_GENERATE_COUNT.fetch_add(1, Ordering::SeqCst);
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

        otel::record_sherpa_tts_synth_wall_ms(wall_start.elapsed().as_secs_f64() * 1000.0, attrs);
        voice_debug(format!(
            "tts synthesis done wall_ms={} audio_duration_ms={}",
            wall_start.elapsed().as_millis(),
            chunk.duration_ms
        ));
        Ok(chunk)
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

pub fn tts_generate_count() -> usize {
    TTS_GENERATE_COUNT.load(Ordering::SeqCst)
}

pub fn reset_tts_generate_count() {
    TTS_GENERATE_COUNT.store(0, Ordering::SeqCst);
}

#[async_trait]
impl TtsProvider for SherpaTts {
    fn vendor_name(&self) -> &'static str {
        "local-sherpa"
    }

    fn bind_session_context(&self, ctx: &VoiceSessionContext) {
        let project_id = ctx
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("")
            .to_string();
        if let Ok(mut guard) = self.project_id.lock() {
            *guard = project_id;
        }
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let normalized = normalize_phrase_text(text);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let model_dir = self.resolved_model_dir().await?;
        let project_id = self.session_project_id();
        let language = self.language_label();
        let voice = self.voice_label();
        let cache_key = build_cache_key(&project_id, &model_dir, &language, &voice, &normalized);
        let attrs = build_metric_attrs(&project_id, &model_dir, &language, &voice);

        if phrase_cache_enabled() {
            if let Some(chunk) = lookup(&cache_key, &attrs) {
                voice_debug(format!(
                    "tts phrase cache hit text_len={} project_id={project_id}",
                    normalized.len()
                ));
                return Ok(vec![chunk]);
            }
        }

        otel::record_sherpa_tts_phrase_cache_miss(&attrs);
        let chunk = self.synthesize_miss(&normalized, &attrs).await?;
        if phrase_cache_enabled() {
            store(cache_key, chunk.clone(), &attrs);
        }
        Ok(vec![chunk])
    }
}
