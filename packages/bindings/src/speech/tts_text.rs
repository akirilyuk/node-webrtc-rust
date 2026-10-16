//! Engine-input text helper for traces (no synthesis).

use napi_derive::napi;
use node_webrtc_rust_speech::config::SentenceCase;
use node_webrtc_rust_speech::tts_text::prepare_engine_text;

/// The text a Sherpa TTS engine receives for a reply, and how many letters were uppercased.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsTtsEngineText {
    pub text: String,
    pub adjusted: u32,
}

/// Exact engine text for `text` under `mode` (`"auto"` default, or `"off"`). Does not synthesize.
///
/// Whitespace is normalised first, as the Sherpa TTS adapters do before the engine sees the text.
#[napi]
pub fn tts_engine_text(text: String, mode: Option<String>) -> JsTtsEngineText {
    let mode = mode
        .as_deref()
        .map_or(SentenceCase::Auto, SentenceCase::from_wire);
    let normalized = node_webrtc_rust_speech::tts_cache::normalize_phrase_text(&text);
    let engine = prepare_engine_text(&normalized, mode);
    JsTtsEngineText {
        text: engine.text.into_owned(),
        adjusted: engine.adjusted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_capitalizes_and_counts() {
        let got = tts_engine_text("Okay. eight one six three ten".into(), None);
        assert_eq!(got.text, "Okay. Eight one six three ten");
        assert_eq!(got.adjusted, 1);
    }

    #[test]
    fn off_and_tiny_fragments_are_unchanged() {
        let off = tts_engine_text("Okay. eight one".into(), Some("off".into()));
        assert_eq!((off.text.as_str(), off.adjusted), ("Okay. eight one", 0));
        let tiny = tts_engine_text("I. am. happy.".into(), Some("auto".into()));
        assert_eq!((tiny.text.as_str(), tiny.adjusted), ("I. am. happy.", 0));
    }

    #[test]
    fn whitespace_is_normalised_like_the_adapters() {
        let got = tts_engine_text("  Okay.   eight one  ".into(), None);
        assert_eq!(got.text, "Okay. Eight one");
        assert_eq!(got.adjusted, 1);
    }
}
