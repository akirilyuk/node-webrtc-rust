//! Resolution of the customer-selected speech transport against a vendor's catalog entry.

use crate::config::SpeechTransport;
use crate::error::{SpeechError, SpeechResult};

/// Resolve the requested transport for one vendor leg.
///
/// - `None` or `auto`: the catalog `default` (`Ok(None)` when the vendor has no selectable transport).
/// - a value in `allowed`: that value.
/// - anything else: a [`SpeechError::Config`] naming the allowed values, so a bad value fails at
///   provider construction and not on the first utterance.
///
/// `vendor` is the catalog id, `leg` is `"stt"` or `"tts"`. `allowed` and `default` come from
/// `voice_catalog::{stt,tts}_transports` / `default_{stt,tts}_transport`.
pub fn resolve_transport(
    vendor: &str,
    leg: &str,
    requested: Option<SpeechTransport>,
    allowed: &[&str],
    default: Option<&str>,
) -> SpeechResult<Option<SpeechTransport>> {
    match requested {
        None | Some(SpeechTransport::Auto) => match default {
            None => Ok(None),
            Some(d) => match SpeechTransport::parse(d) {
                Some(t) if t != SpeechTransport::Auto => Ok(Some(t)),
                _ => Err(SpeechError::Config(format!(
                    "{vendor} {leg} catalog default transport '{d}' is not a concrete transport"
                ))),
            },
        },
        Some(t) => {
            if allowed.is_empty() {
                return Err(SpeechError::Config(format!(
                    "{vendor} {leg} transport is decided by the model; use auto"
                )));
            }
            if allowed.contains(&t.as_str()) {
                Ok(Some(t))
            } else {
                Err(SpeechError::Config(format!(
                    "{vendor} {leg} transport '{}' is not supported; allowed: auto, {}",
                    t.as_str(),
                    allowed.join(", ")
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: &[&str] = &["websocket", "http"];

    fn msg(r: SpeechResult<Option<SpeechTransport>>) -> String {
        match r {
            Err(SpeechError::Config(m)) => m,
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn none_maps_to_catalog_default() {
        let r = resolve_transport("deepgram", "tts", None, BOTH, Some("websocket")).unwrap();
        assert_eq!(r, Some(SpeechTransport::Websocket));
    }

    #[test]
    fn auto_maps_to_catalog_default() {
        let r = resolve_transport(
            "elevenlabs",
            "tts",
            Some(SpeechTransport::Auto),
            BOTH,
            Some("http"),
        )
        .unwrap();
        assert_eq!(r, Some(SpeechTransport::Http));
    }

    #[test]
    fn auto_without_default_is_none() {
        assert_eq!(
            resolve_transport("google", "tts", Some(SpeechTransport::Auto), &[], None).unwrap(),
            None
        );
        assert_eq!(
            resolve_transport("google", "tts", None, &[], None).unwrap(),
            None
        );
    }

    #[test]
    fn allowed_value_is_returned() {
        let r = resolve_transport(
            "deepgram",
            "tts",
            Some(SpeechTransport::Http),
            BOTH,
            Some("websocket"),
        )
        .unwrap();
        assert_eq!(r, Some(SpeechTransport::Http));
    }

    #[test]
    fn unsupported_value_lists_allowed() {
        let m = msg(resolve_transport(
            "assemblyai",
            "stt",
            Some(SpeechTransport::Http),
            &["websocket"],
            Some("websocket"),
        ));
        assert_eq!(
            m,
            "assemblyai stt transport 'http' is not supported; allowed: auto, websocket"
        );
    }

    #[test]
    fn explicit_value_with_empty_allowed_says_model_decides() {
        let m = msg(resolve_transport(
            "google",
            "tts",
            Some(SpeechTransport::Websocket),
            &[],
            None,
        ));
        assert_eq!(m, "google tts transport is decided by the model; use auto");
    }

    #[test]
    fn invalid_catalog_default_is_a_config_error() {
        let m = msg(resolve_transport(
            "x",
            "tts",
            None,
            BOTH,
            Some("carrier-pigeon"),
        ));
        assert!(m.contains("carrier-pigeon"), "{m}");
    }

    #[test]
    fn serde_roundtrip_is_lowercase_and_unknown_is_rejected() {
        let t: SpeechTransport = serde_json::from_str("\"websocket\"").unwrap();
        assert_eq!(t, SpeechTransport::Websocket);
        assert_eq!(
            serde_json::to_string(&SpeechTransport::Http).unwrap(),
            "\"http\""
        );
        assert!(serde_json::from_str::<SpeechTransport>("\"grpc\"").is_err());
    }
}
