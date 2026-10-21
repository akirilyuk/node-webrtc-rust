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
const GOOGLE_TTS_VOICES_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/voice-catalog/catalog/google-tts-voices.json"
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
    #[serde(default)]
    tts_voices_by_model: HashMap<String, Vec<String>>,
    #[serde(default)]
    tts_voice_ids: Option<String>,
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
    #[serde(default)]
    stt_transports: Vec<String>,
    #[serde(default)]
    tts_transports: Vec<String>,
    default_stt_transport: Option<String>,
    default_tts_transport: Option<String>,
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
    pub default_stt_transport: Option<&'static str>,
    pub default_tts_transport: Option<&'static str>,
    stt_transports: &'static [&'static str],
    tts_transports: &'static [&'static str],
    stt_models: &'static [&'static str],
    tts_models: &'static [&'static str],
    tts_voices: &'static [&'static str],
    tts_voices_by_model: HashMap<&'static str, &'static [&'static str]>,
    tts_voice_ids: Option<&'static str>,
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

    /// Transports the customer may pick for STT (`websocket` / `http`). Empty when the model decides.
    pub fn stt_transports(&self) -> &'static [&'static str] {
        self.stt_transports
    }

    /// Transports the customer may pick for TTS (`websocket` / `http`). Empty when the voice or model decides.
    pub fn tts_transports(&self) -> &'static [&'static str] {
        self.tts_transports
    }

    /// Voices documented for one TTS model, when the catalog has a per-model list.
    pub fn tts_voices_by_model(&self, model: &str) -> Option<&'static [&'static str]> {
        self.tts_voices_by_model.get(model).copied()
    }

    /// How voice ids are described by the vendor (`listed`, `model`, `open`).
    pub fn tts_voice_ids(&self) -> Option<&'static str> {
        self.tts_voice_ids
    }
}

struct Catalog {
    vendors: Vec<VendorEntry>,
    by_id: HashMap<&'static str, usize>,
    azure_tts_voices: &'static [&'static str],
    google_tts_voices: &'static [&'static str],
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
        let google_file: AzureVoicesFile = serde_json::from_str(GOOGLE_TTS_VOICES_JSON)
            .expect("google-tts-voices.json must parse");
        let google_tts_voices = leak_str_slice(&google_file.voices);

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
                default_stt_transport: raw.default_stt_transport.map(leak_str),
                default_tts_transport: raw.default_tts_transport.map(leak_str),
                stt_transports: leak_str_slice(&raw.stt_transports),
                tts_transports: leak_str_slice(&raw.tts_transports),
                stt_models: leak_str_slice(&raw.stt_models),
                tts_models: leak_str_slice(&raw.tts_models),
                tts_voices: leak_str_slice(&raw.tts_voices),
                tts_voices_by_model: raw
                    .tts_voices_by_model
                    .iter()
                    .map(|(model, voices)| (leak_str(model.clone()), leak_str_slice(voices)))
                    .collect(),
                tts_voice_ids: raw.tts_voice_ids.map(leak_str),
            };
            by_id.insert(id, vendors.len());
            vendors.push(entry);
        }

        Box::leak(Box::new(Catalog {
            vendors,
            by_id,
            azure_tts_voices,
            google_tts_voices,
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

/// Google Cloud TTS voice names from `google-tts-voices.json` (not duplicated in `vendors.json`).
pub fn google_tts_voices() -> &'static [&'static str] {
    catalog().google_tts_voices
}

/// Voices valid for `vendor_id` and `model`: the per-model list when the catalog has one,
/// else the azure/google snapshot, else the vendor list. `None` for an unknown vendor.
pub fn tts_voices_for_model(
    vendor_id: &str,
    model: Option<&str>,
) -> Option<&'static [&'static str]> {
    let v = vendor(vendor_id)?;
    if let Some(list) = model.and_then(|m| v.tts_voices_by_model(m)) {
        return Some(list);
    }
    match vendor_id {
        "azure" => Some(azure_tts_voices()),
        "google" => Some(google_tts_voices()),
        _ => Some(v.tts_voices()),
    }
}

pub fn tts_voice_ids(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.tts_voice_ids())
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

/// Selectable STT transports for a vendor (empty when none, or the vendor is unknown).
pub fn stt_transports(vendor_id: &str) -> &'static [&'static str] {
    vendor(vendor_id).map(|v| v.stt_transports()).unwrap_or(&[])
}

/// Selectable TTS transports for a vendor (empty when none, or the vendor is unknown).
pub fn tts_transports(vendor_id: &str) -> &'static [&'static str] {
    vendor(vendor_id).map(|v| v.tts_transports()).unwrap_or(&[])
}

/// Transport used by `auto` for STT (today's behaviour). `None` when the vendor has no selectable STT transport.
pub fn default_stt_transport(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.default_stt_transport)
}

/// Transport used by `auto` for TTS (today's behaviour). `None` when the vendor has no selectable TTS transport.
pub fn default_tts_transport(vendor_id: &str) -> Option<&'static str> {
    vendor(vendor_id).and_then(|v| v.default_tts_transport)
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
    fn deepgram_lists_two_tts_transports() {
        assert_eq!(tts_transports("deepgram"), &["websocket", "http"]);
        assert_eq!(default_tts_transport("deepgram"), Some("websocket"));
        assert_eq!(stt_transports("deepgram"), &["websocket"]);
    }

    /// Defaults from the 2026-10-19 vendor bench (`benchmarks/METHODOLOGY.md`, "Default transport rule").
    #[test]
    fn tts_transport_defaults_follow_the_vendor_bench() {
        assert_eq!(default_tts_transport("elevenlabs"), Some("websocket"));
        assert_eq!(default_tts_transport("cartesia"), Some("http"));
        assert_eq!(default_tts_transport("fishaudio"), Some("http"));
        assert_eq!(default_stt_transport("elevenlabs"), Some("websocket"));
    }

    #[test]
    fn mistral_catalog_entry() {
        assert_eq!(
            default_tts_model("mistral"),
            Some("voxtral-mini-tts-3-0")
        );
        assert_eq!(default_tts_voice("mistral"), Some("voice_478"));
        assert_eq!(
            default_stt_model("mistral"),
            Some("voxtral-mini-transcribe-realtime-3-0")
        );
        assert_eq!(tts_voice_ids("mistral"), Some("open"));
        assert_eq!(tts_transports("mistral"), &["http"]);
        assert_eq!(default_tts_transport("mistral"), Some("http"));
        assert_eq!(stt_transports("mistral"), &["websocket"]);
        assert_eq!(default_stt_transport("mistral"), Some("websocket"));
        assert_eq!(
            tts_models("mistral"),
            Some(
                &[
                    "voxtral-mini-tts-3-0",
                    "voxtral-mini-tts-3",
                    "voxtral-mini-tts-realtime-3-0"
                ][..]
            )
        );
    }

    #[test]
    fn google_has_no_selectable_transport() {
        assert!(tts_transports("google").is_empty());
        assert!(stt_transports("google").is_empty());
        assert_eq!(default_tts_transport("google"), None);
        assert_eq!(default_stt_transport("google"), None);
    }

    #[test]
    fn unknown_vendor_has_no_transports() {
        assert!(tts_transports("not-a-vendor").is_empty());
        assert!(stt_transports("not-a-vendor").is_empty());
        assert_eq!(default_tts_transport("not-a-vendor"), None);
    }

    #[test]
    fn default_transport_is_in_list_for_every_vendor() {
        for v in catalog().vendors.iter() {
            for (leg, list, def) in [
                ("tts", v.tts_transports(), v.default_tts_transport),
                ("stt", v.stt_transports(), v.default_stt_transport),
            ] {
                for t in list {
                    assert!(
                        matches!(*t, "websocket" | "http"),
                        "{} {leg} transport {t}",
                        v.id
                    );
                }
                match def {
                    Some(d) => assert!(
                        list.contains(&d),
                        "{} default {leg} transport {d} not in {list:?}",
                        v.id
                    ),
                    None => assert!(
                        list.is_empty(),
                        "{} has {leg} transports but no default",
                        v.id
                    ),
                }
            }
        }
    }

    #[test]
    fn google_snapshot_has_neural2_voice() {
        assert!(google_tts_voices().contains(&"en-US-Neural2-A"));
        let via = tts_voices_for_model("google", None).expect("google");
        assert!(via.contains(&"en-US-Neural2-A"));
    }

    #[test]
    fn openai_tts1_lacks_marin() {
        let tts1 = tts_voices_for_model("openai", Some("tts-1")).expect("openai");
        assert!(!tts1.contains(&"marin"));
        assert!(tts1.contains(&"alloy"));
    }

    #[test]
    fn groq_arabic_list_has_fahad() {
        let ar =
            tts_voices_for_model("groq", Some("canopylabs/orpheus-arabic-saudi")).expect("groq");
        assert!(ar.contains(&"fahad"));
    }

    #[test]
    fn unknown_vendor_voices_is_none() {
        assert!(tts_voices_for_model("not-a-vendor", None).is_none());
    }

    #[test]
    fn unknown_vendor_is_none() {
        assert!(vendor("not-a-vendor").is_none());
    }
}
