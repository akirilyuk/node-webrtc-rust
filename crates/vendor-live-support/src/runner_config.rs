//! The voice agent config the runner builds for a session on staging, for the full-stack live
//! tests ([`crate::fullstack::run_fullstack_with_config`]).
//!
//! Source: the runner log line `voice agent config resolved` of an ElevenLabs session on staging,
//! 2026-10-19 (nwr 0.11.6). The AssemblyAI session had the same shape. Secrets are not part of it.
//!
//! The generic [`crate::vad_gate::preset_vad_config`] differs from it: it has `minSilenceDurationMs`
//! 1300 (runner 1100), `sttGateHoldMs` 400 and `threshold` 0.15 (runner 0.05), `minSpeech` 250
//! (runner 200), and neither language ID nor noise suppression, nor `bargeIn.requireSttPartial`.

use std::path::PathBuf;

use node_webrtc_rust_speech::config::{
    BargeInConfig, EventDeliveryMode, EventsConfig, LanguageIdConfig, LanguageIdTiming,
    NoiseSuppressionConfig, NoiseSuppressionProvider, SttConfig, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::stt_hold::ReplayConfig;
use node_webrtc_rust_speech::VendorRegistry;

/// Directory name of the Whisper tiny language-ID model.
pub const WHISPER_TINY_DIR: &str = "sherpa-onnx-whisper-tiny";

/// Where the Whisper tiny model lives: `$NWR_LID_MODEL_DIR`, else `$NWR_CI_SHERPA_MODELS/<name>`,
/// else the example's `.models` directory in this checkout. `None` when none of them has the
/// encoder.
pub fn whisper_tiny_model_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = crate::env::optional_env("NWR_LID_MODEL_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(root) = crate::env::optional_env("NWR_CI_SHERPA_MODELS") {
        candidates.push(PathBuf::from(root).join(WHISPER_TINY_DIR));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/voice-agent-local-sherpa/.models")
            .join(WHISPER_TINY_DIR),
    );
    candidates.into_iter().find(|dir| {
        dir.join("tiny-encoder.int8.onnx").is_file() || dir.join("tiny-encoder.onnx").is_file()
    })
}

/// The VAD block of the runner's session config on staging (`voice agent config resolved`).
/// The STT gate of the per-vendor live measurements ([`crate::vad_gate::gate_vad_config`]) uses
/// exactly this, so the audio a vendor test pushes is the audio the runner pushes.
pub fn runner_vad_config() -> VadConfig {
    VadConfig {
        enabled: true,
        provider: "energy".into(),
        gate_stt: true,
        gate_stt_open_on_pending: true,
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: 1100,
        speech_pad_ms: 500,
        stt_gate_hold_ms: 400,
        stt_listen_timeout_ms: 4000,
        utterance_finalize_timeout_ms: 1500,
        barge_in: BargeInConfig {
            enabled: true,
            use_vad: true,
            require_stt_partial: true,
            min_stt_partial_tokens: 2,
            flush_tts: true,
            agent_playback_guard_ms: 0,
        },
        ..VadConfig::default()
    }
}

/// The runner's config for `stt` (`model`, `language`, key and the rest of the vendor block are the
/// caller's). Panics, naming the missing piece, when language ID cannot run: the staging bug may
/// depend on it, so a run without it must not pass as a reproduction.
pub fn runner_staging_config(stt: SttConfig) -> VoiceAgentConfig {
    let model_dir = whisper_tiny_model_dir().unwrap_or_else(|| {
        panic!(
            "runner staging config needs the Whisper tiny language-ID model ({WHISPER_TINY_DIR}); \
             set NWR_LID_MODEL_DIR or NWR_CI_SHERPA_MODELS (scripts/ci/ensure-sherpa-models.sh)"
        )
    });
    runner_staging_config_with_lid(stt, Some(model_dir))
}

/// [`runner_staging_config`] with the LID model chosen by the caller (`None` = no LID, which is not
/// the runner's config; for comparison runs only).
pub fn runner_staging_config_with_lid(
    stt: SttConfig,
    lid_model_dir: Option<PathBuf>,
) -> VoiceAgentConfig {
    VoiceAgentConfig {
        stt: Some(stt),
        tts: None,
        language_id: lid_model_dir.map(|dir| LanguageIdConfig {
            enabled: None,
            model_path: Some(dir.to_string_lossy().into_owned()),
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            timing: Some(LanguageIdTiming::Early),
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: None,
        }),
        noise_suppression: NoiseSuppressionConfig {
            provider: NoiseSuppressionProvider::Rnnoise,
        },
        replay: ReplayConfig {
            max_age_ms: Some(60_000),
        },
        events: EventsConfig {
            mode: EventDeliveryMode::Both,
        },
        vad: runner_vad_config(),
        ..VoiceAgentConfig::default()
    }
}

/// Registers the local Sherpa factory, which serves spoken-language ID. Panics without the
/// `sherpa-lid` feature: a config that asks for LID must not silently run without it.
pub fn register_lid(registry: &mut VendorRegistry) {
    #[cfg(feature = "sherpa-lid")]
    {
        use node_webrtc_rust_speech::config::SttVendor;
        use std::sync::Arc;
        registry.register_stt(
            SttVendor::LocalSherpa,
            Arc::new(node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory),
        );
    }
    #[cfg(not(feature = "sherpa-lid"))]
    {
        let _ = registry;
        panic!(
            "the config enables language ID but vendor-live-support was built without the \
             `sherpa-lid` feature; add `--features node-webrtc-rust-vendor-live-support/sherpa-lid`"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    fn stt() -> SttConfig {
        SttConfig {
            provider: SttVendor::Elevenlabs,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
            transport: None,
        }
    }

    #[test]
    fn config_has_the_runner_values() {
        let c = runner_staging_config_with_lid(stt(), Some("/m".into()));
        assert_eq!(c.vad.min_silence_duration_ms, 1100);
        assert_eq!(c.vad.stt_listen_timeout_ms, 4000);
        assert_eq!(c.vad.utterance_finalize_timeout_ms, 1500);
        assert!(c.vad.gate_stt && c.vad.gate_stt_open_on_pending);
        assert!(c.vad.barge_in.require_stt_partial);
        assert_eq!(
            c.noise_suppression.provider,
            NoiseSuppressionProvider::Rnnoise
        );
        assert_eq!(c.replay.max_age_ms, Some(60_000));
        assert_eq!(
            c.language_id.as_ref().and_then(|l| l.timing),
            Some(LanguageIdTiming::Early)
        );
    }
}
