//! Helpers shared by the `stt_replay` and `phrase_sweep` examples: the Sherpa online recognizer
//! with the voice agent's settings, WAV reading, feeding a recognizer, and the offline TTS engine
//! with nwr's settings.

#![allow(dead_code)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use node_webrtc_rust_vendor_sherpa_onnx::{resolve_tts_model_paths, TtsModelFamily};
use sherpa_onnx::{GenerationConfig, OfflineTts, OnlineRecognizer, OnlineRecognizerConfig};

/// Recognizer input rate.
pub const SAMPLE_RATE: i32 = 16_000;
/// 20 ms of 16 kHz audio.
pub const CHUNK_SAMPLES: usize = 320;

fn find_onnx(dir: &Path, keyword: &str) -> Result<String, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let lower = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if path.is_file() && lower.ends_with(".onnx") && lower.contains(keyword) {
            return Ok(path.to_string_lossy().into_owned());
        }
    }
    Err(format!("no {keyword} .onnx in {}", dir.display()))
}

/// The recognizer settings replicate `create_online_recognizer` in `src/loader.rs` (transducer
/// encoder/decoder/joiner/tokens from the model dir, `greedy_search`, endpoint off, 1 thread);
/// that loader is crate-private.
pub fn create_recognizer(model_dir: &Path) -> Result<OnlineRecognizer, String> {
    let tokens = model_dir.join("tokens.txt");
    if !tokens.is_file() {
        return Err(format!("no tokens.txt in {}", model_dir.display()));
    }
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.num_threads = 1;
    config.model_config.transducer.encoder = Some(find_onnx(model_dir, "encoder")?);
    config.model_config.transducer.decoder = Some(find_onnx(model_dir, "decoder")?);
    config.model_config.transducer.joiner = Some(find_onnx(model_dir, "joiner")?);
    config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
    config.enable_endpoint = false;
    config.decoding_method = Some("greedy_search".into());
    OnlineRecognizer::create(&config).ok_or_else(|| "failed to create OnlineRecognizer".to_string())
}

/// Writes f32 `samples` as a 16 kHz mono s16le PCM WAV (the format [`read_wav`] reads).
pub fn write_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Samples of a 16 kHz mono s16le PCM WAV, normalized to f32.
pub fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|e| format!("read: {e}"))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12;
    let mut fmt_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            if body + 16 > bytes.len() {
                return Err("truncated fmt chunk".into());
            }
            let format = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            if format != 1 || channels != 1 || rate != 16_000 || bits != 16 {
                return Err(format!(
                    "expected 16 kHz mono s16le PCM, got format={format} channels={channels} rate={rate} bits={bits}"
                ));
            }
            fmt_ok = true;
        } else if id == b"data" {
            if !fmt_ok {
                return Err("data chunk before fmt chunk".into());
            }
            let end = (body + size).min(bytes.len());
            return Ok(bytes[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
                .collect());
        }
        pos = body + size + (size & 1);
    }
    Err("no data chunk".into())
}

pub fn recognize(recognizer: &OnlineRecognizer, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    for chunk in samples.chunks(CHUNK_SAMPLES) {
        stream.accept_waveform(SAMPLE_RATE, chunk);
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
    }
    // Same tail padding as the product's `finalize_utterance`, so replays decode the last word
    // the way the agent does.
    let tail = vec![
        0.0_f32;
        SAMPLE_RATE as usize
            * node_webrtc_rust_vendor_sherpa_onnx::FINALIZE_TAIL_PADDING_MS
            / 1000
    ];
    stream.accept_waveform(SAMPLE_RATE, &tail);
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    recognizer
        .get_result(&stream)
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default()
}

/// The offline TTS engine exactly as nwr builds it: the public `create_offline_tts` in
/// `src/loader.rs` (VITS `noise_scale` 0.667, `noise_scale_w` 0.8, `length_scale` 1.0, rule FSTs and
/// espeak data resolved from the model dir, `SHERPA_TTS_NUM_THREADS` or 2 threads).
pub fn create_tts(model_dir: &Path) -> Result<OfflineTts, String> {
    use node_webrtc_rust_speech::config::{SentenceCase, TtsConfig, TtsVendor};
    let config = TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_dir.to_string_lossy().into_owned()),
        voice: None,
        api_key: None,
        endpoint: None,
        transport: None,
        sentence_case: SentenceCase::Auto,
    };
    node_webrtc_rust_vendor_sherpa_onnx::create_offline_tts(&config).map_err(|e| e.to_string())
}

/// Per-synthesis settings nwr passes to `generate_with_config` in `src/tts.rs`
/// (`synthesize_sentence`): speaker id from `parse_speaker_id` (0 when the voice is unset) and
/// speed from `parse_speed` (`SHERPA_TTS_SPEED`, else 1.0, clamped to 0.2..=2.0).
pub fn generation_config() -> GenerationConfig {
    generation_config_sid(0)
}

/// [`generation_config`] for speaker `sid` (multi-speaker Piper bundles such as libritts_r).
pub fn generation_config_sid(sid: i32) -> GenerationConfig {
    let speed = std::env::var("SHERPA_TTS_SPEED")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1.0)
        .clamp(0.2, 2.0);
    GenerationConfig {
        sid,
        speed,
        ..Default::default()
    }
}

/// Synthesize `text` with the engine calls nwr's Sherpa TTS makes for it: whitespace
/// normalization (`normalize_phrase_text`), then [`engine_pieces`] (sentence-start capitalization
/// under `SentenceCase::Auto`, split into sentences with a short first piece), one engine call per
/// piece, samples concatenated back to back as `synthesize_miss` streams them. Returns the samples
/// and the engine sample rate.
///
/// [`engine_pieces`]: node_webrtc_rust_vendor_sherpa_onnx::engine_pieces
pub fn synthesize(
    tts: &OfflineTts,
    text: &str,
    family: TtsModelFamily,
) -> Result<(Vec<f32>, i32), String> {
    synthesize_family(tts, text, 0, family)
}

/// The model family of the bundle in `model_dir`, as nwr resolves it.
pub fn tts_family(model_dir: &Path) -> Result<TtsModelFamily, String> {
    use node_webrtc_rust_speech::config::{SentenceCase, TtsConfig, TtsVendor};
    let config = TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_dir.to_string_lossy().into_owned()),
        voice: None,
        api_key: None,
        endpoint: None,
        transport: None,
        sentence_case: SentenceCase::Auto,
    };
    resolve_tts_model_paths(&config)
        .map(|paths| paths.family)
        .map_err(|e| e.to_string())
}

/// [`synthesize`] with speaker id `sid` for a VITS bundle (same `GenerationConfig` otherwise).
pub fn synthesize_sid(tts: &OfflineTts, text: &str, sid: i32) -> Result<(Vec<f32>, i32), String> {
    synthesize_family(tts, text, sid, TtsModelFamily::Vits)
}

/// [`synthesize`] with speaker id `sid` for a bundle of `family`.
pub fn synthesize_family(
    tts: &OfflineTts,
    text: &str,
    sid: i32,
    family: TtsModelFamily,
) -> Result<(Vec<f32>, i32), String> {
    use node_webrtc_rust_speech::config::SentenceCase;
    use node_webrtc_rust_speech::tts_cache::normalize_phrase_text;
    use node_webrtc_rust_vendor_sherpa_onnx::engine_pieces;
    let mut samples = Vec::new();
    for piece in engine_pieces(&normalize_phrase_text(text), SentenceCase::Auto, family) {
        let audio = tts
            .generate_with_config(
                &piece,
                &generation_config_sid(sid),
                None::<fn(&[f32], f32) -> bool>,
            )
            .ok_or_else(|| format!("OfflineTts generation returned no audio for {piece:?}"))?;
        samples.extend_from_slice(audio.samples());
    }
    Ok((samples, tts.sample_rate()))
}

/// Linear-interpolation resample of mono f32 `samples` from `from_hz` to `to_hz`. Output length
/// is `round(len * to_hz / from_hz)`. No anti-alias filter: the bench voices are band-limited
/// speech and the same conversion is applied to every clip.
pub fn resample_linear(samples: &[f32], from_hz: i32, to_hz: i32) -> Vec<f32> {
    if from_hz == to_hz || samples.is_empty() {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as f64) * f64::from(to_hz) / f64::from(from_hz)).round() as usize;
    let step = f64::from(from_hz) / f64::from(to_hz);
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * step;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let a = samples[idx.min(samples.len() - 1)];
            let b = samples[(idx + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Level of voiced speech in dBFS: mean RMS of the 20 ms frames whose RMS is above -45 dBFS,
/// expressed in dB. `f32::NEG_INFINITY` when no frame is voiced.
pub fn rms_voiced_db(samples: &[f32], sample_rate: i32) -> f32 {
    let frame = (sample_rate as usize / 50).max(1);
    let floor = 10f32.powf(-45.0 / 20.0);
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for chunk in samples.chunks_exact(frame) {
        let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / frame as f32).sqrt();
        if rms > floor {
            sum += f64::from(rms);
            count += 1;
        }
    }
    if count == 0 {
        return f32::NEG_INFINITY;
    }
    20.0 * ((sum / count as f64) as f32).log10()
}

/// Writes mono f32 `samples` at 48 kHz as a 48 kHz **stereo** s16le WAV (the same audio in both
/// channels): what the voice agent receives from WebRTC.
pub fn write_wav_stereo_48k(path: &Path, samples: &[f32]) -> Result<(), String> {
    let data_len = (samples.len() * 4) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Raw s16le stereo PCM bytes of a 48 kHz stereo WAV written by [`write_wav_stereo_48k`].
pub fn read_wav_stereo_48k_pcm(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|e| format!("read: {e}"))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    if channels != 2 || rate != 48_000 || &bytes[36..40] != b"data" {
        return Err(format!(
            "expected 48 kHz stereo s16le, got channels={channels} rate={rate}"
        ));
    }
    Ok(bytes[44..].to_vec())
}

/// Lowercase hex sha256.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (slot, value) in h.iter_mut().zip(v) {
            *slot = slot.wrapping_add(value);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

// Minimal JSON reader (the example crate has no serde_json): enough for the sweep and manifest files.

#[derive(Debug, Clone)]
#[allow(dead_code)] // `Bool` is parsed but no field of the sweep file is read as a bool.
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn expect(&mut self, byte: u8) -> Result<(), String> {
        self.ws();
        if self.bytes.get(self.pos) == Some(&byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!(
                "json: expected '{}' at byte {}",
                byte as char, self.pos
            ))
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.bytes.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                let mut fields = Vec::new();
                loop {
                    self.ws();
                    if self.bytes.get(self.pos) == Some(&b'}') {
                        self.pos += 1;
                        break;
                    }
                    let key = self.string()?;
                    self.expect(b':')?;
                    fields.push((key, self.value()?));
                    self.ws();
                    if self.bytes.get(self.pos) == Some(&b',') {
                        self.pos += 1;
                    }
                }
                Ok(Json::Obj(fields))
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.bytes.get(self.pos) == Some(&b']') {
                        self.pos += 1;
                        break;
                    }
                    items.push(self.value()?);
                    self.ws();
                    if self.bytes.get(self.pos) == Some(&b',') {
                        self.pos += 1;
                    }
                }
                Ok(Json::Arr(items))
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') if self.bytes[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(Json::Bool(true))
            }
            Some(b'f') if self.bytes[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(Json::Bool(false))
            }
            Some(b'n') if self.bytes[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(Json::Null)
            }
            Some(_) => {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && matches!(
                        self.bytes[self.pos],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.pos += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.pos])
                    .ok()
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(Json::Num)
                    .ok_or_else(|| format!("json: bad value at byte {start}"))
            }
            None => Err("json: unexpected end".into()),
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or("json: unterminated string")?;
            self.pos += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let esc = *self.bytes.get(self.pos).ok_or("json: bad escape")?;
                    self.pos += 1;
                    match esc {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(
                                self.bytes
                                    .get(self.pos..self.pos + 4)
                                    .ok_or("json: bad \\u")?,
                            )
                            .map_err(|e| e.to_string())?;
                            let code = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                            self.pos += 4;
                            let ch = char::from_u32(code).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        String::from_utf8(out).map_err(|e| e.to_string())
    }
}

pub fn parse_json(text: &str) -> Result<Json, String> {
    JsonParser {
        bytes: text.as_bytes(),
        pos: 0,
    }
    .value()
}

pub fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// `normalizeForCompare` of the roundtrip oracle (`roundtrip-counting.ts`), same as `phrase_sweep`:
/// lowercase, every char outside `[a-z0-9\s]` becomes a space, whitespace runs collapse to one
/// space, trimmed.
pub fn normalize_for_compare(text: &str) -> String {
    let lowered = text.to_lowercase();
    let replaced: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || is_js_whitespace(c) {
                c
            } else {
                ' '
            }
        })
        .collect();
    replaced
        .split(is_js_whitespace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// JS `\s` (also the whitespace `trim` removes).
fn is_js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `wordSimilarity` of the roundtrip oracle, same as `phrase_sweep`: share of the input's words
/// found as whole tokens in the recognized text.
pub fn word_similarity(input: &str, recognized: &str) -> f64 {
    let normalized_input = normalize_for_compare(input);
    let words: Vec<&str> = normalized_input
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return if recognized.trim().is_empty() {
            0.0
        } else {
            1.0
        };
    }
    let haystack = format!(" {} ", normalize_for_compare(recognized));
    let hits = words
        .iter()
        .filter(|word| haystack.contains(&format!(" {word} ")))
        .count();
    hits as f64 / words.len() as f64
}
