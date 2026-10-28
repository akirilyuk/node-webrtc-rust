use std::fs;
use std::path::{Path, PathBuf};

use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

/// Model family of a Sherpa offline TTS bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsModelFamily {
    Vits,
    Kokoro,
    Kitten,
}

/// Resolved Sherpa offline TTS (VITS/Piper, Kokoro, Kitten) files under a model directory.
#[derive(Debug, Clone)]
pub struct ResolvedTtsModelPaths {
    #[allow(dead_code)]
    pub model_dir: PathBuf,
    pub family: TtsModelFamily,
    /// The `.onnx` model file (VITS, Kokoro or Kitten).
    pub model: PathBuf,
    /// voices.bin (Kokoro and Kitten only).
    pub voices: Option<PathBuf>,
    /// Lexicon files for Kokoro, in the order they are passed (comma-joined).
    pub lexicons: Vec<PathBuf>,
    pub tokens: PathBuf,
    /// espeak-ng-data (Piper). None for lexicon models without espeak data.
    pub data_dir: Option<PathBuf>,
    /// lexicon.txt (Melo and other lexicon VITS models).
    pub lexicon: Option<PathBuf>,
    /// dict/ next to the lexicon (jieba dictionary for Chinese Melo).
    pub dict_dir: Option<PathBuf>,
    /// Rule FSTs shipped with the model, in the documented order.
    pub rule_fsts: Vec<PathBuf>,
}

/// Rule FSTs in the order the sherpa-onnx Melo docs pass them.
const DOCUMENTED_RULE_FSTS: [&str; 4] =
    ["phone.fst", "date.fst", "number.fst", "new_heteronym.fst"];

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

    if model_dir.join("voices.bin").is_file() {
        return resolve_voices_bundle(model_dir, tokens);
    }

    let vits_model = find_vits_onnx(&model_dir)?;

    let lexicon_path = model_dir.join("lexicon.txt");
    if !lexicon_path.is_file() {
        let data_dir = find_espeak_data_dir(&model_dir)?;
        return Ok(ResolvedTtsModelPaths {
            model_dir,
            tokens,
            family: TtsModelFamily::Vits,
            model: vits_model,
            voices: None,
            lexicons: Vec::new(),
            data_dir: Some(data_dir),
            lexicon: None,
            dict_dir: None,
            rule_fsts: Vec::new(),
        });
    }

    let dict_path = model_dir.join("dict");
    let dict_dir = dict_path.is_dir().then_some(dict_path);
    let rule_fsts = find_rule_fsts(&model_dir)?;
    let data_dir = find_espeak_data_dir(&model_dir).ok();

    Ok(ResolvedTtsModelPaths {
        model_dir,
        tokens,
        family: TtsModelFamily::Vits,
        model: vits_model,
        voices: None,
        lexicons: Vec::new(),
        data_dir,
        lexicon: Some(lexicon_path),
        dict_dir,
        rule_fsts,
    })
}

/// Kokoro / Kitten bundle: `voices.bin` next to the model, family from the directory name.
/// No rule FSTs are applied: the Kokoro multi-lang bundle ships Chinese normalizers only.
fn resolve_voices_bundle(
    model_dir: PathBuf,
    tokens: PathBuf,
) -> SpeechResult<ResolvedTtsModelPaths> {
    let dir_name = model_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    let family = if dir_name.starts_with("kokoro") {
        TtsModelFamily::Kokoro
    } else if dir_name.starts_with("kitten") {
        TtsModelFamily::Kitten
    } else {
        return Err(SpeechError::Config(format!(
            "TTS model dir has voices.bin but is neither kokoro-* nor kitten-*: {}",
            model_dir.display()
        )));
    };

    let model = ["model.onnx", "model.fp32.onnx", "model.int8.onnx"]
        .iter()
        .map(|name| model_dir.join(name))
        .find(|path| path.is_file() && is_real_model_file(path))
        .ok_or_else(|| {
            SpeechError::Config(format!(
                "no Kokoro/Kitten model.onnx in {}",
                model_dir.display()
            ))
        })?;
    let data_dir = find_espeak_data_dir(&model_dir)?;

    let (lexicons, dict_dir) = if family == TtsModelFamily::Kokoro {
        let lexicons: Vec<PathBuf> = ["lexicon-us-en.txt", "lexicon-zh.txt"]
            .iter()
            .map(|name| model_dir.join(name))
            .filter(|path| path.is_file())
            .collect();
        let dict_path = model_dir.join("dict");
        (lexicons, dict_path.is_dir().then_some(dict_path))
    } else {
        (Vec::new(), None)
    };

    Ok(ResolvedTtsModelPaths {
        voices: Some(model_dir.join("voices.bin")),
        model_dir,
        family,
        model,
        lexicons,
        tokens,
        data_dir: Some(data_dir),
        lexicon: None,
        dict_dir,
        rule_fsts: Vec::new(),
    })
}

/// Documented rule FSTs that exist, then any other `*.fst` sorted by name.
fn find_rule_fsts(dir: &Path) -> SpeechResult<Vec<PathBuf>> {
    let mut fsts = Vec::new();
    for name in DOCUMENTED_RULE_FSTS {
        let path = dir.join(name);
        if path.is_file() {
            fsts.push(path);
        }
    }

    let mut others = Vec::new();
    for entry in read_dir(dir)? {
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
        if name.to_lowercase().ends_with(".fst") && !DOCUMENTED_RULE_FSTS.contains(&name) {
            others.push(path);
        }
    }
    others.sort();
    fsts.extend(others);
    Ok(fsts)
}

fn resolve_tts_model_dir(config: &TtsConfig) -> SpeechResult<PathBuf> {
    if let Some(path) = config.model_path.as_ref().filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    std::env::var("SHERPA_TTS_MODEL_PATH")
        .map(PathBuf::from)
        .map_err(|_| SpeechError::Config("missing TTS model_path or SHERPA_TTS_MODEL_PATH".into()))
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

/// Files this small are git-lfs pointers (about 130 bytes), never real ONNX models.
const MIN_ONNX_MODEL_BYTES: u64 = 1024;

fn is_real_model_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.len() > MIN_ONNX_MODEL_BYTES)
}

fn find_vits_onnx(dir: &Path) -> SpeechResult<PathBuf> {
    let preferred = dir.join("model.onnx");
    if preferred.is_file() && is_real_model_file(&preferred) {
        return Ok(preferred);
    }

    let entries = read_dir(dir)?;
    let mut candidates = Vec::new();
    let mut skipped_pointers = false;

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
        // A tiny .onnx is a git-lfs pointer, not a model; loading it aborts ORT.
        if !is_real_model_file(&path) {
            skipped_pointers = true;
            continue;
        }
        candidates.push(path);
    }

    candidates.sort_by_key(|path| path.file_name().map(|name| name.to_owned()));
    candidates.into_iter().next().ok_or_else(|| {
        let suffix = if skipped_pointers {
            " (only git-lfs pointer files found)"
        } else {
            ""
        };
        SpeechError::Config(format!(
            "no VITS/Piper .onnx model found in {}{suffix}",
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
        SpeechError::Config(format!(
            "failed to read model directory {}: {err}",
            dir.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn named(name: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "nwr-tts-paths-{}-{}-{n}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let model = dir.join(name);
            fs::create_dir_all(&model).unwrap();
            Self(model)
        }

        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "nwr-tts-paths-{}-{}-{n}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            // A dedicated child so a sibling `espeak-ng-data` next to the model dir can never exist.
            let model = dir.join("model");
            fs::create_dir_all(&model).unwrap();
            Self(model)
        }

        fn file(&self, name: &str) {
            self.file_sized(name, 2048);
        }

        fn file_sized(&self, name: &str, len: usize) {
            fs::write(self.0.join(name), vec![b'x'; len]).unwrap();
        }

        fn dir(&self, name: &str) {
            fs::create_dir_all(self.0.join(name)).unwrap();
        }

        fn config(&self) -> TtsConfig {
            TtsConfig {
                provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
                model: None,
                model_path: Some(self.0.to_str().unwrap().to_string()),
                voice: None,
                api_key: None,
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn piper_dir_requires_espeak_and_has_no_lexicon() {
        let dir = TempDir::new();
        dir.file("tokens.txt");
        dir.file("model.onnx");
        dir.dir("espeak-ng-data");

        let paths = resolve_tts_model_paths(&dir.config()).unwrap();
        assert_eq!(paths.data_dir, Some(dir.0.join("espeak-ng-data")));
        assert!(paths.lexicon.is_none());
        assert!(paths.dict_dir.is_none());
        assert!(paths.rule_fsts.is_empty());
    }

    #[test]
    fn piper_dir_without_espeak_or_lexicon_keeps_existing_error() {
        let dir = TempDir::new();
        dir.file("tokens.txt");
        dir.file("model.onnx");

        let err = resolve_tts_model_paths(&dir.config()).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("no espeak-ng-data directory in"),
            "unexpected error: {text}"
        );
        assert!(text.contains("set SHERPA_TTS_DATA_DIR"), "{text}");
    }

    #[test]
    fn melo_dir_resolves_lexicon_dict_and_ordered_rule_fsts() {
        let dir = TempDir::new();
        for name in [
            "tokens.txt",
            "model.onnx",
            "lexicon.txt",
            "date.fst",
            "phone.fst",
            "number.fst",
            "new_heteronym.fst",
            "extra.fst",
        ] {
            dir.file(name);
        }
        dir.dir("dict");

        let paths = resolve_tts_model_paths(&dir.config()).unwrap();
        assert!(paths.data_dir.is_none());
        assert_eq!(paths.lexicon, Some(dir.0.join("lexicon.txt")));
        assert_eq!(paths.dict_dir, Some(dir.0.join("dict")));
        let expected: Vec<PathBuf> = [
            "phone.fst",
            "date.fst",
            "number.fst",
            "new_heteronym.fst",
            "extra.fst",
        ]
        .iter()
        .map(|name| dir.0.join(name))
        .collect();
        assert_eq!(paths.rule_fsts, expected);
    }

    #[test]
    fn melo_dir_prefers_model_onnx_over_int8_pointer() {
        let dir = TempDir::new();
        dir.file("tokens.txt");
        dir.file("lexicon.txt");
        dir.file_sized("model.int8.onnx", 133);
        dir.file_sized("model.onnx", 2048);

        let paths = resolve_tts_model_paths(&dir.config()).unwrap();
        assert_eq!(paths.model, dir.0.join("model.onnx"));
    }

    #[test]
    fn pointer_only_onnx_is_rejected() {
        let dir = TempDir::new();
        dir.file("tokens.txt");
        dir.dir("espeak-ng-data");
        dir.file_sized("x.onnx", 133);

        let err = resolve_tts_model_paths(&dir.config()).unwrap_err();
        assert!(err.to_string().contains("git-lfs pointer"), "{err}");
    }

    #[test]
    fn kokoro_dir_resolves_family_voices_lexicons_dict_no_fsts() {
        let dir = TempDir::named("kokoro-multi-lang-v1_0");
        for name in [
            "model.onnx",
            "voices.bin",
            "tokens.txt",
            "lexicon-us-en.txt",
            "lexicon-zh.txt",
            "number-zh.fst",
        ] {
            dir.file(name);
        }
        dir.dir("espeak-ng-data");
        dir.dir("dict");

        let paths = resolve_tts_model_paths(&dir.config()).unwrap();
        assert_eq!(paths.family, TtsModelFamily::Kokoro);
        assert_eq!(paths.voices, Some(dir.0.join("voices.bin")));
        assert_eq!(
            paths.lexicons,
            vec![
                dir.0.join("lexicon-us-en.txt"),
                dir.0.join("lexicon-zh.txt")
            ]
        );
        assert_eq!(paths.dict_dir, Some(dir.0.join("dict")));
        assert!(paths.lexicon.is_none());
        assert!(paths.rule_fsts.is_empty());
    }

    #[test]
    fn kitten_dir_picks_model_fp32_onnx() {
        let dir = TempDir::named("kitten-nano-en-v0_8-fp32");
        for name in ["model.fp32.onnx", "voices.bin", "tokens.txt"] {
            dir.file(name);
        }
        dir.dir("espeak-ng-data");

        let paths = resolve_tts_model_paths(&dir.config()).unwrap();
        assert_eq!(paths.family, TtsModelFamily::Kitten);
        assert_eq!(paths.model, dir.0.join("model.fp32.onnx"));
        assert!(paths.lexicons.is_empty());
        assert!(paths.dict_dir.is_none());
    }

    #[test]
    fn voices_bin_in_unknown_dir_is_an_error() {
        let dir = TempDir::named("mystery-tts");
        for name in ["model.onnx", "voices.bin", "tokens.txt"] {
            dir.file(name);
        }
        dir.dir("espeak-ng-data");

        let err = resolve_tts_model_paths(&dir.config()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("neither kokoro-* nor kitten-*"), "{text}");
        assert!(text.contains("mystery-tts"), "{text}");
    }

    #[test]
    fn kitten_without_espeak_data_is_an_error() {
        let dir = TempDir::named("kitten-nano-en-v0_8-fp32");
        for name in ["model.fp32.onnx", "voices.bin", "tokens.txt"] {
            dir.file(name);
        }

        let err = resolve_tts_model_paths(&dir.config()).unwrap_err();
        assert!(
            err.to_string().contains("no espeak-ng-data directory in"),
            "{err}"
        );
    }
}
