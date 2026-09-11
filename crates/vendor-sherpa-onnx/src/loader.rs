//! Sherpa ONNX model construction (shared by pool and tests).

use std::sync::atomic::{AtomicUsize, Ordering};

use node_webrtc_rust_speech::config::{LanguageIdConfig, SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use sherpa_onnx::{
    OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
    OnlineRecognizer, OnlineRecognizerConfig, SpokenLanguageIdentification,
    SpokenLanguageIdentificationConfig, SpokenLanguageIdentificationWhisperConfig,
};

use crate::lid_model_paths::{lid_paths_to_strings, resolve_lid_model_paths};
use crate::model_paths::resolve_model_paths;
use crate::tts_model_paths::resolve_tts_model_paths;

pub(crate) fn stt_num_threads() -> i32 {
    parse_thread_env("SHERPA_STT_NUM_THREADS").unwrap_or(1)
}

pub(crate) fn lid_num_threads() -> i32 {
    parse_thread_env("SHERPA_LID_NUM_THREADS").unwrap_or(1)
}

fn tts_num_threads() -> i32 {
    parse_thread_env("SHERPA_TTS_NUM_THREADS").unwrap_or(2)
}

pub(crate) fn parse_thread_env(name: &str) -> Option<i32> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&limit| limit > 0)
        .map(|value| value.min(i32::MAX as usize) as i32)
}

fn parse_f32_env(name: &str) -> Option<f32> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn parse_bool_env(name: &str) -> Option<bool> {
    match std::env::var(name)
        .ok()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

static STT_RECOGNIZER_CREATE_COUNT: AtomicUsize = AtomicUsize::new(0);
static TTS_ENGINE_CREATE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn create_online_recognizer(config: &SttConfig) -> SpeechResult<OnlineRecognizer> {
    let paths = resolve_model_paths(config)?;

    let mut recognizer_config = OnlineRecognizerConfig::default();
    recognizer_config.model_config.num_threads = stt_num_threads();
    recognizer_config.model_config.transducer.encoder = Some(path_to_string(&paths.encoder)?);
    recognizer_config.model_config.transducer.decoder = Some(path_to_string(&paths.decoder)?);
    recognizer_config.model_config.transducer.joiner = Some(path_to_string(&paths.joiner)?);
    recognizer_config.model_config.tokens = Some(path_to_string(&paths.tokens)?);
    // VoiceAgent drives utterance boundaries via VAD + finalize_utterance(). Sherpa's
    // built-in endpoint during gate-hold silence caused duplicate finals and repeated
    // words when combined with our endpoint tail. Opt in with SHERPA_STT_ENABLE_ENDPOINT=1.
    let enable_endpoint = parse_bool_env("SHERPA_STT_ENABLE_ENDPOINT").unwrap_or(false);
    recognizer_config.enable_endpoint = enable_endpoint;
    if enable_endpoint {
        recognizer_config.rule1_min_trailing_silence =
            parse_f32_env("SHERPA_STT_RULE1_MIN_TRAILING_SILENCE").unwrap_or(2.4);
        recognizer_config.rule2_min_trailing_silence =
            parse_f32_env("SHERPA_STT_RULE2_MIN_TRAILING_SILENCE").unwrap_or(1.0);
        recognizer_config.rule3_min_utterance_length =
            parse_f32_env("SHERPA_STT_RULE3_MIN_UTTERANCE_LENGTH").unwrap_or(20.0);
    }
    recognizer_config.decoding_method = Some("greedy_search".into());

    let recognizer =
        OnlineRecognizer::create(&recognizer_config).ok_or_else(|| SpeechError::Vendor {
            vendor: "local-sherpa".into(),
            message: "failed to create OnlineRecognizer".into(),
        })?;

    STT_RECOGNIZER_CREATE_COUNT.fetch_add(1, Ordering::SeqCst);
    Ok(recognizer)
}

pub fn create_offline_tts(config: &TtsConfig) -> SpeechResult<OfflineTts> {
    let paths = resolve_tts_model_paths(config)?;

    let mut model_config = OfflineTtsModelConfig::default();
    model_config.num_threads = tts_num_threads();
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

    TTS_ENGINE_CREATE_COUNT.fetch_add(1, Ordering::SeqCst);
    Ok(tts)
}

pub fn create_spoken_language_identification(
    config: &LanguageIdConfig,
) -> SpeechResult<SpokenLanguageIdentification> {
    let paths = resolve_lid_model_paths(config)?;
    let (encoder, decoder) = lid_paths_to_strings(&paths)?;

    let whisper = SpokenLanguageIdentificationWhisperConfig {
        encoder: Some(encoder),
        decoder: Some(decoder),
        tail_paddings: 0,
    };
    let lid_config = SpokenLanguageIdentificationConfig {
        whisper,
        num_threads: lid_num_threads(),
        debug: false,
        provider: Some("cpu".to_string()),
    };

    SpokenLanguageIdentification::create(&lid_config).ok_or_else(|| SpeechError::Vendor {
        vendor: "local-sherpa".into(),
        message: "failed to create SpokenLanguageIdentification — check languageId.modelPath"
            .into(),
    })
}

pub fn path_to_string(path: &std::path::Path) -> SpeechResult<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        SpeechError::Config(format!("model path is not valid UTF-8: {}", path.display()))
    })
}

pub fn stt_recognizer_create_count() -> usize {
    STT_RECOGNIZER_CREATE_COUNT.load(Ordering::SeqCst)
}

pub fn tts_engine_create_count() -> usize {
    TTS_ENGINE_CREATE_COUNT.load(Ordering::SeqCst)
}

pub fn reset_create_counters() {
    STT_RECOGNIZER_CREATE_COUNT.store(0, Ordering::SeqCst);
    TTS_ENGINE_CREATE_COUNT.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod thread_env_tests {
    use super::{lid_num_threads, parse_thread_env, stt_num_threads};

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn lid_and_stt_thread_env_defaults_and_override_independently() {
        {
            let _lid = EnvGuard::unset("SHERPA_LID_NUM_THREADS");
            let _stt = EnvGuard::unset("SHERPA_STT_NUM_THREADS");
            assert_eq!(lid_num_threads(), 1);
            assert_eq!(stt_num_threads(), 1);
        }
        {
            let _lid = EnvGuard::set("SHERPA_LID_NUM_THREADS", "3");
            let _stt = EnvGuard::set("SHERPA_STT_NUM_THREADS", "2");
            assert_eq!(lid_num_threads(), 3);
            assert_eq!(stt_num_threads(), 2);
            assert_eq!(parse_thread_env("SHERPA_LID_NUM_THREADS"), Some(3));
            assert_eq!(parse_thread_env("SHERPA_STT_NUM_THREADS"), Some(2));
        }
    }
}
