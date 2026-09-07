//! Resolve Whisper encoder/decoder paths for spoken language identification.

use std::path::{Path, PathBuf};

use node_webrtc_rust_speech::config::LanguageIdConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

use crate::loader::path_to_string;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LidModelPaths {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
}

const ENCODER_CANDIDATES: &[&str] = &[
    "tiny-encoder.int8.onnx",
    "encoder.int8.onnx",
    "tiny-encoder.onnx",
    "encoder.onnx",
];

const DECODER_CANDIDATES: &[&str] = &[
    "tiny-decoder.int8.onnx",
    "decoder.int8.onnx",
    "tiny-decoder.onnx",
    "decoder.onnx",
];

fn resolve_in_dir(dir: &Path, candidates: &[&str]) -> Option<PathBuf> {
    for name in candidates {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn resolve_lid_model_dir(config: &LanguageIdConfig) -> SpeechResult<PathBuf> {
    let raw = config
        .model_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            SpeechError::Config("languageId.modelPath is required for spoken language ID".into())
        })?;
    Ok(PathBuf::from(raw))
}

pub fn resolve_lid_model_paths(config: &LanguageIdConfig) -> SpeechResult<LidModelPaths> {
    let dir = resolve_lid_model_dir(config)?;
    if !dir.is_dir() {
        return Err(SpeechError::Config(format!(
            "languageId.modelPath is not a directory: {}",
            dir.display()
        )));
    }

    let encoder = resolve_in_dir(&dir, ENCODER_CANDIDATES).ok_or_else(|| {
        SpeechError::Config(format!(
            "missing Whisper encoder ONNX in {} (expected one of: {})",
            dir.display(),
            ENCODER_CANDIDATES.join(", ")
        ))
    })?;
    let decoder = resolve_in_dir(&dir, DECODER_CANDIDATES).ok_or_else(|| {
        SpeechError::Config(format!(
            "missing Whisper decoder ONNX in {} (expected one of: {})",
            dir.display(),
            DECODER_CANDIDATES.join(", ")
        ))
    })?;

    Ok(LidModelPaths { encoder, decoder })
}

pub fn lid_pool_key(config: &LanguageIdConfig) -> SpeechResult<PathBuf> {
    resolve_lid_model_dir(config)
}

pub fn lid_paths_to_strings(paths: &LidModelPaths) -> SpeechResult<(String, String)> {
    Ok((
        path_to_string(&paths.encoder)?,
        path_to_string(&paths.decoder)?,
    ))
}
