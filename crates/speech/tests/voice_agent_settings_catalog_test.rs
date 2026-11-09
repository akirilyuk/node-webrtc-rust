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
    // `speakerGate.threshold` is checked by `speaker_gate_threshold_bounds_match_validate`.
    let validated = [
        "vad.adaptive.noiseMarginDb",
        "vad.adaptive.stuckActiveMs",
        "speakerGate.threshold",
    ];
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

/// An enabled gate (with a model path) after setting each `(path, value)` of `values`.
fn gate_with(values: &[(&str, Value)]) -> Result<(), String> {
    let mut v = base();
    set_path(&mut v, "speakerGate.enabled", json!(true));
    set_path(
        &mut v,
        "speakerGate.modelPath",
        json!("/models/titanet.onnx"),
    );
    for (path, value) in values {
        set_path(&mut v, path, value.clone());
    }
    let cfg: VoiceAgentConfig = serde_json::from_value(v).map_err(|e| e.to_string())?;
    cfg.speaker_gate.validate()
}

#[test]
fn speaker_gate_threshold_bounds_match_validate() {
    let file = catalog();
    let def = &settings(&file)["speakerGate.threshold"];
    let (min, max) = (def["min"].as_f64().unwrap(), def["max"].as_f64().unwrap());
    let excl_min = def["exclusiveMin"] == json!(true);
    let excl_max = def["exclusiveMax"] == json!(true);
    let at = |x: f64| gate_with(&[("speakerGate.threshold", json!(x))]).is_ok();
    assert_eq!(
        at(min),
        !excl_min,
        "threshold at min {min}: exclusiveMin={excl_min} disagrees with validate()"
    );
    assert_eq!(
        at(max),
        !excl_max,
        "threshold at max {max}: exclusiveMax={excl_max} disagrees with validate()"
    );
    assert!(at(0.001), "threshold 0.001 rejected");
    assert!(at(0.999), "threshold 0.999 rejected");
    assert!(!at(-0.1), "threshold below min accepted");
    assert!(!at(1.1), "threshold above max accepted");
    // Only the threshold carries exclusive flags.
    for (path, d) in settings(&file) {
        if path != "speakerGate.threshold" {
            assert!(
                d.get("exclusiveMin").is_none() && d.get("exclusiveMax").is_none(),
                "{path}: exclusive flag on a key the engine does not validate that way"
            );
        }
    }
}

/// Whether the catalog constraint `c` fails for `values` (a path missing from it takes the
/// catalog default).
fn constraint_fails(file: &Value, c: &Value, values: &[(&str, Value)]) -> bool {
    let value_of = |path: &str| -> Value {
        values
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| settings(file)[path]["default"].clone())
    };
    if let Some(when) = c.get("when") {
        let any = when["anyTrue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| value_of(p.as_str().unwrap()) == json!(true));
        if !any {
            return false;
        }
    }
    let value = value_of(c["path"].as_str().unwrap());
    if let Some(ne) = c.get("notEqual") {
        if &value == ne {
            return true;
        }
    }
    if let Some(other) = c.get("gte") {
        let other = value_of(other.as_str().unwrap());
        if value.as_f64().unwrap() < other.as_f64().unwrap() {
            return true;
        }
    }
    false
}

#[test]
fn constraints_match_validate_on_every_combination() {
    let file = catalog();
    let constraints = file["constraints"].as_array().expect("constraints array");
    assert_eq!(
        constraints.len(),
        2,
        "one catalog constraint per validate() cross-field rule"
    );
    for c in constraints {
        let id = c["id"].as_str().unwrap();
        assert!(
            settings(&file).contains_key(c["path"].as_str().unwrap()),
            "{id}: unknown path"
        );
        assert!(
            c["message"]
                .as_str()
                .is_some_and(|m| m.contains(c["path"].as_str().unwrap())),
            "{id}: message must name the path"
        );
    }
    let mut failing_cases = 0;
    for recheck in [true, false] {
        for split in [true, false] {
            for windows in [0u32, 1, 2] {
                for (check, min) in [(1000u32, 1500u32), (1500, 1500), (2000, 1500), (1000, 1000)] {
                    let values = [
                        ("speakerGate.recheckOnOther", json!(recheck)),
                        ("speakerGate.splitOnSpeakerChange", json!(split)),
                        ("speakerGate.recheckMaxWindows", json!(windows)),
                        ("speakerGate.checkSpeechMs", json!(check)),
                        ("speakerGate.minCheckSpeechMs", json!(min)),
                    ];
                    let failing: Vec<&Value> = constraints
                        .iter()
                        .filter(|c| constraint_fails(&file, c, &values))
                        .collect();
                    let result = gate_with(&values);
                    let case = format!(
                        "recheck={recheck} split={split} windows={windows} check={check} min={min}"
                    );
                    assert_eq!(
                        result.is_err(),
                        !failing.is_empty(),
                        "{case}: validate()={result:?} but catalog constraints fail {:?}",
                        failing
                            .iter()
                            .map(|c| c["id"].as_str().unwrap())
                            .collect::<Vec<_>>()
                    );
                    if let Err(msg) = result {
                        failing_cases += 1;
                        assert!(
                            failing.iter().any(|c| msg.contains(c["path"].as_str().unwrap())),
                            "{case}: validate() message `{msg}` names none of the failing constraint paths"
                        );
                        if failing.len() == 1 {
                            assert!(
                                msg.contains(failing[0]["path"].as_str().unwrap()),
                                "{case}: {msg}"
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(failing_cases > 0, "the grid never hit a failing case");
}

#[test]
fn constraint_named_cases() {
    // R1 fails for (recheckOnOther, 1) and (split only, 1); passes for (both off, 1) and (on, 0/2).
    let w = |recheck: bool, split: bool, n: u32| {
        gate_with(&[
            ("speakerGate.recheckOnOther", json!(recheck)),
            ("speakerGate.splitOnSpeakerChange", json!(split)),
            ("speakerGate.recheckMaxWindows", json!(n)),
        ])
    };
    assert!(w(true, false, 1).is_err());
    assert!(w(false, true, 1).is_err());
    assert!(w(false, false, 1).is_ok());
    assert!(w(true, false, 0).is_ok());
    assert!(w(true, false, 2).is_ok());
    // R2: fails at check 1000 / min 1500, passes when equal.
    let c = |check: u32| gate_with(&[("speakerGate.checkSpeechMs", json!(check))]);
    assert!(c(1000).is_err());
    assert!(c(1500).is_ok());
}
