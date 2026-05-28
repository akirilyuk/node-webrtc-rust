use std::fs;
use std::path::{Path, PathBuf};

use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

/// Resolved ONNX transducer model files under a model directory.
#[derive(Debug, Clone)]
pub struct ResolvedModelPaths {
    #[allow(dead_code)]
    pub model_dir: PathBuf,
    pub tokens: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
}

pub fn resolve_model_paths(config: &SttConfig) -> SpeechResult<ResolvedModelPaths> {
    let model_dir = resolve_model_dir(config)?;
    if !model_dir.is_dir() {
        return Err(SpeechError::Config(format!(
            "model path is not a directory: {}",
            model_dir.display()
        )));
    }

    let tokens = find_tokens(&model_dir)?;
    let encoder = find_onnx_with_keyword(&model_dir, "encoder")?;
    let decoder = find_onnx_with_keyword(&model_dir, "decoder")?;
    let joiner = find_onnx_with_keyword(&model_dir, "joiner")?;

    Ok(ResolvedModelPaths {
        model_dir,
        tokens,
        encoder,
        decoder,
        joiner,
    })
}

fn resolve_model_dir(config: &SttConfig) -> SpeechResult<PathBuf> {
    if let Some(path) = config.model_path.as_ref().filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    if let Ok(path) = std::env::var("SHERPA_STT_MODEL_PATH") {
        return Ok(PathBuf::from(path));
    }

    if let Ok(path) = std::env::var("SHERPA_MODEL_PATH") {
        return Ok(PathBuf::from(path));
    }

    Err(SpeechError::Config(
        "missing STT model_path or SHERPA_STT_MODEL_PATH".into(),
    ))
}

fn find_tokens(dir: &Path) -> SpeechResult<PathBuf> {
    let exact = dir.join("tokens.txt");
    if exact.is_file() {
        return Ok(exact);
    }

    let entries = fs::read_dir(dir).map_err(|err| {
        SpeechError::Config(format!("failed to read model directory {}: {err}", dir.display()))
    })?;

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

fn find_onnx_with_keyword(dir: &Path, keyword: &str) -> SpeechResult<PathBuf> {
    let entries = fs::read_dir(dir).map_err(|err| {
        SpeechError::Config(format!("failed to read model directory {}: {err}", dir.display()))
    })?;

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
        if lower.ends_with(".onnx") && lower.contains(keyword) {
            return Ok(path);
        }
    }

    Err(SpeechError::Config(format!(
        "no {keyword} .onnx model found in {}",
        dir.display()
    )))
}
