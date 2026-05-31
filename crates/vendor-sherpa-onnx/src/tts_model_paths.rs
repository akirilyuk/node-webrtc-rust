use std::fs;
use std::path::{Path, PathBuf};

use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

/// Resolved Sherpa offline TTS (VITS/Piper) files under a model directory.
#[derive(Debug, Clone)]
pub struct ResolvedTtsModelPaths {
    #[allow(dead_code)]
    pub model_dir: PathBuf,
    pub vits_model: PathBuf,
    pub tokens: PathBuf,
    pub data_dir: PathBuf,
}

/// Model directory from config/env only (no ONNX file validation).
pub fn resolve_tts_model_dir_path(config: &TtsConfig) -> SpeechResult<PathBuf> {
    resolve_tts_model_dir(config)
}

pub fn resolve_tts_model_paths(config: &TtsConfig) -> SpeechResult<ResolvedTtsModelPaths> {
    let model_dir = resolve_tts_model_dir(config)?;
    if !model_dir.is_dir() {
        return Err(SpeechError::Config(format!(
            "TTS model path is not a directory: {}",
            model_dir.display()
        )));
    }

    let tokens = find_tokens(&model_dir)?;
    let vits_model = find_vits_onnx(&model_dir)?;
    let data_dir = find_espeak_data_dir(&model_dir)?;

    Ok(ResolvedTtsModelPaths {
        model_dir,
        tokens,
        vits_model,
        data_dir,
    })
}

fn resolve_tts_model_dir(config: &TtsConfig) -> SpeechResult<PathBuf> {
    if let Some(path) = config.model_path.as_ref().filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    std::env::var("SHERPA_TTS_MODEL_PATH")
        .map(PathBuf::from)
        .map_err(|_| {
            SpeechError::Config("missing TTS model_path or SHERPA_TTS_MODEL_PATH".into())
        })
}

fn find_tokens(dir: &Path) -> SpeechResult<PathBuf> {
    let exact = dir.join("tokens.txt");
    if exact.is_file() {
        return Ok(exact);
    }

    let entries = read_dir(dir)?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            SpeechError::Config(format!("failed to read model directory entry: {err}"))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower = name.to_lowercase();
        if lower.ends_with(".txt") && lower.contains("tokens") {
            return Ok(path);
        }
    }

    Err(SpeechError::Config(format!(
        "no tokens.txt found in {}",
        dir.display()
    )))
}

fn find_vits_onnx(dir: &Path) -> SpeechResult<PathBuf> {
    let entries = read_dir(dir)?;
    let mut candidates = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|err| {
            SpeechError::Config(format!("failed to read model directory entry: {err}"))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let lower = name.to_lowercase();
        if !lower.ends_with(".onnx") {
            continue;
        }
        if lower.contains("encoder")
            || lower.contains("decoder")
            || lower.contains("joiner")
            || lower.contains("vocoder")
        {
            continue;
        }
        candidates.push(path);
    }

    candidates.sort_by_key(|path| path.file_name().map(|name| name.to_owned()));
    candidates.into_iter().next().ok_or_else(|| {
        SpeechError::Config(format!(
            "no VITS/Piper .onnx model found in {}",
            dir.display()
        ))
    })
}

fn find_espeak_data_dir(model_dir: &Path) -> SpeechResult<PathBuf> {
    let nested = model_dir.join("espeak-ng-data");
    if nested.is_dir() {
        return Ok(nested);
    }

    if let Ok(path) = std::env::var("SHERPA_TTS_DATA_DIR") {
        let data_dir = PathBuf::from(path);
        if data_dir.is_dir() {
            return Ok(data_dir);
        }
    }

    if let Some(parent) = model_dir.parent() {
        let shared = parent.join("espeak-ng-data");
        if shared.is_dir() {
            return Ok(shared);
        }
    }

    Err(SpeechError::Config(format!(
        "no espeak-ng-data directory in {} — run download-tts (bundles include it) or set SHERPA_TTS_DATA_DIR",
        model_dir.display()
    )))
}

fn read_dir(dir: &Path) -> SpeechResult<fs::ReadDir> {
    fs::read_dir(dir).map_err(|err| {
        SpeechError::Config(format!("failed to read model directory {}: {err}", dir.display()))
    })
}
