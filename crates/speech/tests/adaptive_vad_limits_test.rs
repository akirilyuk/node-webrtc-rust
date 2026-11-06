//! The `@node-webrtc-rust/voice-catalog` adaptive VAD limits must equal what
//! `AdaptiveVadConfig::validate()` accepts. A change on either side fails here.

use node_webrtc_rust_speech::config::AdaptiveVadConfig;
use serde_json::{json, Value};

fn limits() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/voice-catalog/catalog/adaptive-vad-limits.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read adaptive-vad-limits.json"))
        .expect("parse adaptive-vad-limits.json")
}

fn parse(key: &str, value: Value) -> Result<AdaptiveVadConfig, String> {
    let cfg: AdaptiveVadConfig =
        serde_json::from_value(json!({ key: value })).map_err(|e| e.to_string())?;
    cfg.validate()?;
    Ok(cfg)
}

#[test]
fn adaptive_vad_limits_match_catalog() {
    let file = limits();
    let all = &file["limits"];
    assert_eq!(
        all.as_object().unwrap().len(),
        2,
        "unexpected keys in catalog limits"
    );

    // noiseMarginDb: float, accepted at min and max, rejected just outside.
    let n = &all["noiseMarginDb"];
    assert_eq!(n["integer"], json!(false));
    let (min, max) = (n["min"].as_f64().unwrap(), n["max"].as_f64().unwrap());
    assert!(
        parse("noiseMarginDb", json!(min)).is_ok(),
        "min {min} accepted"
    );
    assert!(
        parse("noiseMarginDb", json!(max)).is_ok(),
        "max {max} accepted"
    );
    assert!(
        parse("noiseMarginDb", json!(min - 0.01)).is_err(),
        "below min rejected"
    );
    assert!(
        parse("noiseMarginDb", json!(max + 0.01)).is_err(),
        "above max rejected"
    );

    // stuckActiveMs: integer (u32), accepted at min and max, min - 1 rejected.
    let s = &all["stuckActiveMs"];
    assert_eq!(s["integer"], json!(true));
    let (min, max) = (s["min"].as_u64().unwrap(), s["max"].as_u64().unwrap());
    assert!(
        parse("stuckActiveMs", json!(min)).is_ok(),
        "min {min} accepted"
    );
    assert!(
        parse("stuckActiveMs", json!(max)).is_ok(),
        "max {max} accepted"
    );
    assert!(
        parse("stuckActiveMs", json!(min - 1)).is_err(),
        "min - 1 rejected"
    );
    // The field is a u32, so max + 1 is rejected when the value is read.
    assert_eq!(max, u64::from(u32::MAX), "catalog max is the u32 maximum");
    assert!(
        parse("stuckActiveMs", json!(max + 1)).is_err(),
        "max + 1 rejected"
    );
    // A fraction is not an integer.
    assert!(
        parse("stuckActiveMs", json!(min as f64 + 0.5)).is_err(),
        "fraction rejected"
    );
}

#[test]
fn adaptive_vad_keys_are_config_fields() {
    let serialized = serde_json::to_value(AdaptiveVadConfig::default()).unwrap();
    let fields = serialized.as_object().unwrap();
    let keys = limits()["keys"].as_array().unwrap().clone();
    assert!(!keys.is_empty(), "catalog keys empty");
    for k in keys {
        let k = k.as_str().unwrap();
        assert!(
            fields.contains_key(k),
            "catalog key {k} is not a field of AdaptiveVadConfig"
        );
    }
    // Every limits key is also a listed key.
    for k in limits()["limits"].as_object().unwrap().keys() {
        assert!(
            limits()["keys"].as_array().unwrap().iter().any(|x| x == k),
            "{k} missing from keys"
        );
    }
}
