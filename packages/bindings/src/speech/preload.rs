//! Process-wide model preload entry points (runner boot, no VoiceAgent needed).

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::speech::types::{speech_err, JsLanguageIdConfig};

/// Load the shared spoken-language-ID model (Sherpa Whisper tiny) into the process-wide pool.
///
/// Resolves once the model is resident. Idempotent: every `VoiceAgent` with the same
/// `languageId.modelPath` then reuses this one instance, so the first utterance never pays
/// the load. Rejects when `modelPath` is missing or not a Whisper bundle directory.
#[napi]
pub async fn preload_language_id(config: JsLanguageIdConfig) -> Result<()> {
    node_webrtc_rust_vendor_sherpa_onnx::preload_language_id_async(config.into())
        .await
        .map_err(speech_err)
}
