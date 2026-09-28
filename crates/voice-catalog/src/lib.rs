//! Vendor STT/TTS model and voice allowlists embedded from `@node-webrtc-rust/voice-catalog` JSON.
//!
//! No HTTP, no speech crate — parse once at first use via `include_str!` on the npm package catalog files.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const VENDORS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/voice-catalog/catalog/vendors.json"
));
const AZURE_TTS_VOICES_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/voice-catalog/catalog/azure-tts-voices.json"
));

#[derive(Debug, Deserialize)]
struct VendorsFile {
    vendors: Vec<VendorEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct AzureVoicesFile {
    voices: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VendorEntryRaw {
    id: String,
    label: String,
    stt: bool,
    tts: bool,
    default_stt_model: Option<String>,
    default_tts_model: Option<String>,
    default_tts_voice: Option<String>,
    stt_models: Vec<String>,
    tts_models: Vec<String>,
    tts_voices: Vec<String>,
    home: Option<String>,
    stt_docs: Option<String>,
    tts_docs: Option<String>,
    models_docs: Option<String>,
}

/// One vendor row from `vendors.json` (string fields are leaked for `'static` lifetimes).
#[derive(Debug)]
pub struct VendorEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub stt: bool,
    pub tts: bool,
    pub default_stt_model: Option<&'static str>,
    pub default_tts_model: Option<&'static str>,
    pub default_tts_voice: Option<&'static str>,
    pub home: Option<&'static str>,
    pub stt_docs: Option<&'static str>,
    pub tts_docs: Option<&'static str>,
    pub models_docs: Option<&'static str>,
    stt_models: &'static [&'static str],
    tts_models: &'static [&'static str],
    tts_voices: &'static [&'static str],
}

impl VendorEntry {
    pub fn stt_models(&self) -> &'static [&'static str] {
        self.stt_models
    }

    pub fn tts_models(&self) -> &'static [&'static str] {
        self.tts_models
    }

    pub fn tts_voices(&self) -> &'static [&'static str] {
        self.tts_voices
    }
}

struct Catalog {
    vendors: Vec<VendorEntry>,
    by_id: HashMap<&'static str, usize>,
    azure_tts_voices: &'static [&'static str],
}

static CATALOG: OnceLock<&'static Catalog> = OnceLock::new();

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_str_slice(strings: &[String]) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = strings.iter().map(|s| leak_str(s.clone())).collect();
    Box::leak(leaked.into_boxed_slice())
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        let vendors_file: VendorsFile =
            serde_json::from_str(VENDORS_JSON).expect("vendors.json must parse");
        let azure_file: AzureVoicesFile =
            serde_json::from_str(AZURE_TTS_VOICES_JSON).expect("azure-tts-voices.json must parse");
        let azure_tts_voices = leak_str_slice(&azure_file.voices);

        let mut vendors = Vec::with_capacity(vendors_file.vendors.len());
        let mut by_id = HashMap::new();

        for raw in vendors_file.vendors {
            let id = leak_str(raw.id);
            let entry = VendorEntry {
                id,
                label: leak_str(raw.label),
                stt: raw.stt,
                tts: raw.tts,
                default_stt_model: raw.default_stt_model.map(leak_str),
                default_tts_model: raw.default_tts_model.map(leak_str),
                default_tts_voice: raw.default_tts_voice.map(leak_str),
                home: raw.home.map(leak_str),
                stt_docs: raw.stt_docs.map(leak_str),
                tts_docs: raw.tts_docs.map(leak_str),
                models_docs: raw.models_docs.map(leak_str),
                stt_models: leak_str_slice(&raw.stt_models),
                tts_models: leak_str_slice(&raw.tts_models),
                tts_voices: leak_str_slice(&raw.tts_voices),
            };
            by_id.insert(id, vendors.len());
            vendors.push(entry);
        }

        Box::leak(Box::new(Catalog {
            vendors,
            by_id,
            azure_tts_voices,
        }))
    })
}

/// Lookup vendor by catalog `id` (e.g. `"openai"`, `"groq"`).
pub fn vendor(id: &str) -> Option<&'static VendorEntry> {
    let c = catalog();
    c.by_id.get(id).map(|&i| &c.vendors[i])
}

/// Azure TTS ShortNames from `azure-tts-voices.json` (not duplicated in `vendors.json`).
pub fn azure_tts_voices() -> &'static [&'static str] {
    catalog().azure_tts_voices
}

pub fn stt_models(vendor_id: &str) -> Option<&'static [&'static str]> {
    vendor(vendor_id).map(|v| v.stt_models())
}

pub fn tts_models(vendor_id: &str) -> Option<&'static [&'static str]> {
    vendor(vendor_id).map(|v| v.tts_models())
}

pub fn tts_voices(vendor_id: &str) -> Option<&'static [&'static str]> {
    vendor(vendor_id).map(|v| v.tts_voices())
}

pub fn default_stt_model(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.default_stt_model)
}

pub fn default_tts_model(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.default_tts_model)
}

pub fn default_tts_voice(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.default_tts_voice)
}

pub fn stt_docs_url(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.stt_docs)
}

pub fn tts_docs_url(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.tts_docs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors_json_parses() {
        let _ = catalog();
        assert!(vendor("openai").is_some());
    }

    #[test]
    fn groq_stt_models_match_catalog() {
        let models = stt_models("groq").expect("groq");
        assert_eq!(models.len(), 2);
        assert!(models.contains(&"whisper-large-v3-turbo"));
        assert!(models.contains(&"whisper-large-v3"));
    }

    #[test]
    fn azure_voice_snapshot() {
        let voices = azure_tts_voices();
        assert_eq!(voices.len(), 761);
        assert!(voices.contains(&"en-US-JennyNeural"));
        assert!(voices.contains(&"en-US-Ava:DragonHDLatestNeural"));
    }

    #[test]
    fn unknown_vendor_is_none() {
        assert!(vendor("not-a-vendor").is_none());
    }
}
