//! `@node-webrtc-rust/voice-catalog` `catalog/voice-agent-settings.json` must equal what
//! `VoiceAgentConfig` serializes, defaults to and validates. A change on either side fails here,
//! and every failure names the path.

use node_webrtc_rust_speech::config::{
    language_id_timing, resolved_language_id_min_speech_ms, resolved_post_utterance_silence_ms,
    LanguageIdConfig, VoiceAgentConfig,
};
use serde_json::{json, Map, Value};

const U32_MAX: u64 = u32::MAX as u64;

/// Paths whose Rust field is a `String`, so serde cannot reject an unknown value. Their enum is
/// checked against the branches that read the field instead (`vad.rs`).
const STRING_TYPED_ENUMS: &[&str] = &["vad.provider"];

fn catalog() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/voice-catalog/catalog/voice-agent-settings.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read voice-agent-settings.json"))
        .expect("parse voice-agent-settings.json")
}

fn adaptive_limits() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/voice-catalog/catalog/adaptive-vad-limits.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read adaptive-vad-limits.json"))
        .expect("parse adaptive-vad-limits.json")
}

fn settings(file: &Value) -> &Map<String, Value> {
    file["settings"].as_object().expect("settings object")
}

/// The default config as JSON, with `languageId` present (it is `null` by default) so its keys
/// can be set.
fn base() -> Value {
    let mut v = serde_json::to_value(VoiceAgentConfig::default()).unwrap();
    v["languageId"] = lid_default_json();
    v
}

fn lid_default_json() -> Value {
    let cfg: LanguageIdConfig = serde_json::from_value(json!({})).unwrap();
    serde_json::to_value(cfg).unwrap()
}

fn set_path(root: &mut Value, path: &str, value: Value) {
    let mut cur = root;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cur.as_object_mut()
                .unwrap_or_else(|| panic!("{path}: parent is not an object"))
                .insert(part.to_string(), value);
            return;
        }
        cur = cur
            .get_mut(part)
            .unwrap_or_else(|| panic!("{path}: no field `{part}` in the serialized config"));
    }
}

fn get_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |cur, part| cur.get(part))
}

fn parse_with(path: &str, value: Value) -> Result<VoiceAgentConfig, String> {
    let mut v = base();
    set_path(&mut v, path, value);
    serde_json::from_value(v).map_err(|e| e.to_string())
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        _ => "other",
    }
}

fn close(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-6,
        _ => a == b,
    }
}

/// The engine's effective default of `path`, as the catalog states it.
fn engine_default(path: &str) -> Value {
    let cfg: LanguageIdConfig = serde_json::from_value(json!({})).unwrap();
    match path {
        "languageId.minSpeechMs" => json!(resolved_language_id_min_speech_ms(&cfg)),
        "languageId.timing" => serde_json::to_value(language_id_timing(&Some(cfg))).unwrap(),
        "postUtteranceSilenceMs" => {
            assert!(
                VoiceAgentConfig::default()
                    .post_utterance_silence_ms
                    .is_none(),
                "postUtteranceSilenceMs must default to None (derived)"
            );
            Value::Null
        }
        _ => get_path(&base(), path)
            .unwrap_or_else(|| panic!("{path}: not a serialized field of VoiceAgentConfig"))
            .clone(),
    }
}

#[test]
fn paths_defaults_and_types_match_the_engine() {
    let file = catalog();
    for (path, def) in settings(&file) {
        let engine = engine_default(path);
        let catalog_default = &def["default"];
        assert!(
            close(&engine, catalog_default),
            "{path}: catalog default {catalog_default} != engine default {engine}"
        );
        let ty = def["type"].as_str().expect("type");
        if !catalog_default.is_null() {
            assert_eq!(
                json_kind(catalog_default),
                ty,
                "{path}: default kind does not match type"
            );
        } else {
            assert_eq!(ty, "number", "{path}: a null default is a derived number");
        }
        // The path itself must exist in the serialized config (a Rust rename fails here).
        assert!(
            get_path(&base(), path).is_some(),
            "{path}: not a serialized field of VoiceAgentConfig"
        );
    }
}

#[test]
fn integer_flag_matches_the_field_type() {
    let file = catalog();
    for (path, def) in settings(&file) {
        if def["type"] != "number" {
            assert!(
                def.get("integer").is_none(),
                "{path}: integer on a non-number"
            );
            continue;
        }
        let flag = def["integer"]
            .as_bool()
            .unwrap_or_else(|| panic!("{path}: integer flag missing"));
        let fraction = parse_with(path, json!(1.5));
        assert_eq!(
            fraction.is_ok(),
            !flag,
            "{path}: catalog integer={flag} but the engine {} 1.5",
            if fraction.is_ok() {
                "accepts"
            } else {
                "rejects"
            }
        );
        if flag {
            // u32 fields: -1 and 2^32 are rejected when read.
            assert!(parse_with(path, json!(-1)).is_err(), "{path}: -1 accepted");
            assert!(
                parse_with(path, json!(U32_MAX + 1)).is_err(),
                "{path}: u32 max + 1 accepted"
            );
            assert!(
                parse_with(path, json!(U32_MAX)).is_ok(),
                "{path}: u32 max rejected"
            );
        }
    }
}

#[test]
fn enum_values_are_what_the_engine_accepts() {
    let file = catalog();
    let vad_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/vad.rs"))
        .expect("read vad.rs");
    for (path, def) in settings(&file) {
        let Some(values) = def.get("enum") else {
            continue;
        };
        let values = values.as_array().unwrap();
        assert_eq!(def["type"], "string", "{path}: enum on a non-string");
        assert!(
            values.contains(&def["default"]),
            "{path}: default not in enum"
        );
        for v in values {
            assert!(
                parse_with(path, v.clone()).is_ok(),
                "{path}: engine rejects enum value {v}"
            );
        }
        if STRING_TYPED_ENUMS.contains(&path.as_str()) {
            // A String field: every non-default value must be a branch of the engine that reads it.
            for v in values.iter().filter(|v| **v != def["default"]) {
                let needle = format!("config.provider == {v}");
                assert!(
                    vad_rs.contains(&needle),
                    "{path}: vad.rs has no branch for {v}"
                );
            }
        } else {
            assert!(
                parse_with(path, json!("__invalid__")).is_err(),
                "{path}: engine accepts an invalid enum value"
            );
        }
    }
}

#[test]
fn bounds_are_the_u32_range_or_a_validate_rule() {
    let file = catalog();
    // Bounds enforced by `AdaptiveVadConfig::validate()`.
    let validated = ["vad.adaptive.noiseMarginDb", "vad.adaptive.stuckActiveMs"];
    for (path, def) in settings(&file) {
        let has_bounds = def.get("min").is_some() || def.get("max").is_some();
        if validated.contains(&path.as_str()) {
            continue;
        }
        if def["integer"] == json!(true) {
            assert_eq!(
                def["min"],
                json!(0),
                "{path}: unvalidated integer key must start at 0"
            );
            assert_eq!(
                def["max"],
                json!(U32_MAX),
                "{path}: unvalidated integer key must end at u32 max"
            );
        } else {
            assert!(
                !has_bounds,
                "{path}: bounds on a key the engine does not validate"
            );
        }
    }

    let check = |path: &str, value: Value| -> bool {
        parse_with(path, value)
            .map(|c| c.vad.adaptive.validate().is_ok())
            .unwrap_or(false)
    };

    let n = &settings(&file)["vad.adaptive.noiseMarginDb"];
    let (min, max) = (n["min"].as_f64().unwrap(), n["max"].as_f64().unwrap());
    assert!(
        check("vad.adaptive.noiseMarginDb", json!(min)),
        "noiseMarginDb min {min} rejected"
    );
    assert!(
        check("vad.adaptive.noiseMarginDb", json!(max)),
        "noiseMarginDb max {max} rejected"
    );
    assert!(
        !check("vad.adaptive.noiseMarginDb", json!(min - 0.001)),
        "noiseMarginDb below min accepted"
    );
    assert!(
        !check("vad.adaptive.noiseMarginDb", json!(max + 0.001)),
        "noiseMarginDb above max accepted"
    );

    let s = &settings(&file)["vad.adaptive.stuckActiveMs"];
    let (min, max) = (s["min"].as_u64().unwrap(), s["max"].as_u64().unwrap());
    assert!(
        check("vad.adaptive.stuckActiveMs", json!(min)),
        "stuckActiveMs min {min} rejected"
    );
    assert!(
        check("vad.adaptive.stuckActiveMs", json!(max)),
        "stuckActiveMs max {max} rejected"
    );
    assert!(
        !check("vad.adaptive.stuckActiveMs", json!(min - 1)),
        "stuckActiveMs min - 1 accepted"
    );
    assert!(
        !check("vad.adaptive.stuckActiveMs", json!(max + 1)),
        "stuckActiveMs max + 1 accepted"
    );
}

#[test]
fn adaptive_entries_equal_the_adaptive_limits_file() {
    let file = catalog();
    let limits = adaptive_limits();
    for key in limits["keys"].as_array().unwrap() {
        let key = key.as_str().unwrap();
        assert!(
            settings(&file).contains_key(&format!("vad.adaptive.{key}")),
            "vad.adaptive.{key} missing from voice-agent-settings.json"
        );
    }
    for (key, limit) in limits["limits"].as_object().unwrap() {
        let def = &settings(&file)[&format!("vad.adaptive.{key}")];
        for field in ["min", "max", "integer"] {
            assert_eq!(
                def[field], limit[field],
                "vad.adaptive.{key}.{field} differs from adaptive-vad-limits.json"
            );
        }
    }
}

#[test]
fn presets_deserialize_and_voice_agent_matches_the_sdk_preset() {
    let file = catalog();
    let presets = file["presets"].as_object().unwrap();
    assert_eq!(
        presets.keys().map(String::as_str).collect::<Vec<_>>().len(),
        3,
        "expected default, voiceAgent, browserWebrtc"
    );
    for (name, values) in presets {
        let mut v = base();
        for (path, value) in values.as_object().unwrap() {
            assert!(
                settings(&file).contains_key(path),
                "preset {name}: unknown path {path}"
            );
            set_path(&mut v, path, value.clone());
        }
        let cfg: VoiceAgentConfig = serde_json::from_value(v)
            .unwrap_or_else(|e| panic!("preset {name} does not deserialize: {e}"));
        cfg.vad
            .adaptive
            .validate()
            .unwrap_or_else(|e| panic!("preset {name}: {e}"));
    }
    // `voiceAgent` is the SDK `VOICE_AGENT_VAD_PRESET` over `DEFAULT_VOICE_AGENT_VAD`.
    assert_eq!(presets["voiceAgent"]["vad.gateStt"], json!(true));
    assert_eq!(presets["voiceAgent"]["vad.speechPadMs"], json!(500));
    assert_eq!(presets["voiceAgent"].as_object().unwrap().len(), 2);
    assert!(presets["default"].as_object().unwrap().is_empty());
    // `browserWebrtc` is a generic library fact: the quieter energy threshold.
    assert_eq!(presets["browserWebrtc"]["vad.provider"], json!("energy"));
    assert_eq!(presets["browserWebrtc"]["vad.threshold"], json!(0.05));
    assert_eq!(
        presets["browserWebrtc"]["vad.minSpeechDurationMs"],
        json!(200)
    );
}

#[test]
fn post_utterance_silence_pad_matches_the_engine() {
    let file = catalog();
    let pad = file["postUtteranceSilencePadMs"].as_u64().unwrap() as u32;
    let mut cfg = VoiceAgentConfig {
        post_utterance_silence_ms: None,
        ..Default::default()
    };
    assert_eq!(
        resolved_post_utterance_silence_ms(&cfg),
        cfg.vad.stt_gate_hold_ms + cfg.vad.min_silence_duration_ms + pad,
        "derived post-utterance silence differs from hold + minSilence + postUtteranceSilencePadMs"
    );
    cfg.post_utterance_silence_ms = Some(0);
    assert_eq!(
        resolved_post_utterance_silence_ms(&cfg),
        0,
        "explicit 0 stays 0"
    );
}
