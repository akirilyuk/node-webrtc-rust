use std::sync::Arc;

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
};
use tokio::sync::Mutex;

use crate::audio::f32_mono_to_stereo_48k_s16le;
use crate::tts_model_paths::resolve_tts_model_paths;

struct SherpaTtsEngine {
    tts: OfflineTts,
    speaker_id: i32,
    speed: f32,
}

pub struct SherpaTts {
    config: TtsConfig,
    engine: Arc<Mutex<Option<SherpaTtsEngine>>>,
}

impl SherpaTts {
    pub fn new(config: &TtsConfig) -> Self {
        Self {
            config: config.clone(),
            engine: Arc::new(Mutex::new(None)),
        }
    }

    fn build_engine(config: &TtsConfig) -> SpeechResult<SherpaTtsEngine> {
        let paths = resolve_tts_model_paths(config)?;

        let mut model_config = OfflineTtsModelConfig::default();
        model_config.num_threads = 2;
        model_config.vits = OfflineTtsVitsModelConfig {
            model: Some(path_to_string(&paths.vits_model)?),
            tokens: Some(path_to_string(&paths.tokens)?),
            data_dir: Some(path_to_string(&paths.data_dir)?),
            noise_scale: 0.667,
            noise_scale_w: 0.8,
            length_scale: 1.0,
            ..Default::default()
        };

        let tts_config = OfflineTtsConfig {
            model: model_config,
            ..Default::default()
        };

        let tts = OfflineTts::create(&tts_config).ok_or_else(|| SpeechError::Vendor {
            vendor: "local-sherpa".into(),
            message: "failed to create OfflineTts — check SHERPA_TTS_MODEL_PATH and espeak-ng-data"
                .into(),
        })?;

        Ok(SherpaTtsEngine {
            tts,
            speaker_id: parse_speaker_id(config),
            speed: parse_speed(config),
        })
    }

    async fn ensure_engine(&self) -> SpeechResult<()> {
        let mut guard = self.engine.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let config = self.config.clone();
        let built = tokio::task::spawn_blocking(move || SherpaTts::build_engine(&config))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))??;
        *guard = Some(built);
        Ok(())
    }
}

fn path_to_string(path: &std::path::Path) -> SpeechResult<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| {
            SpeechError::Config(format!(
                "model path is not valid UTF-8: {}",
                path.display()
            ))
        })
}

fn parse_speaker_id(config: &TtsConfig) -> i32 {
    config
        .voice
        .as_deref()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0)
}

fn parse_speed(config: &TtsConfig) -> f32 {
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

        self.ensure_engine().await?;

        let input = trimmed.to_string();
        let engine = Arc::clone(&self.engine);

        let chunk = tokio::task::spawn_blocking(move || -> SpeechResult<TtsAudioChunk> {
            let mut guard = engine.blocking_lock();
            let engine = guard.as_mut().ok_or_else(|| {
                SpeechError::Internal("Sherpa TTS engine missing after init".into())
            })?;

            let gen_config = GenerationConfig {
                sid: engine.speaker_id,
                speed: engine.speed,
                ..Default::default()
            };

            let audio = engine
                .tts
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
