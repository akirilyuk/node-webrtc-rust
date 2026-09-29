//! Speech event NAPI wiring.

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadSafeCallContext, ThreadsafeFunctionCallMode};
use napi::JsFunction;
use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};

use crate::events::create_event_callback;
use crate::speech::types::{JsSpeechEvent, JsSpeechEventType};

pub fn speech_event_to_js(event: SpeechEvent) -> JsSpeechEvent {
    JsSpeechEvent {
        event_type: match event.kind {
            SpeechEventKind::UserSpeakingStart => JsSpeechEventType::UserSpeakingStart,
            SpeechEventKind::UserSpeakingEnd => JsSpeechEventType::UserSpeakingEnd,
            SpeechEventKind::UserSpeechPartial => JsSpeechEventType::UserSpeechPartial,
            SpeechEventKind::UserSpeechFinal => JsSpeechEventType::UserSpeechFinal,
            SpeechEventKind::UserLanguage => JsSpeechEventType::UserLanguage,
            SpeechEventKind::AgentSpeakingStart => JsSpeechEventType::AgentSpeakingStart,
            SpeechEventKind::AgentSpeakingEnd => JsSpeechEventType::AgentSpeakingEnd,
            SpeechEventKind::VadTriggered => JsSpeechEventType::VadTriggered,
            SpeechEventKind::SttStreamStart => JsSpeechEventType::SttStreamStart,
            SpeechEventKind::SttStreamEnd => JsSpeechEventType::SttStreamEnd,
            SpeechEventKind::UserSttStart => JsSpeechEventType::UserSttStart,
            SpeechEventKind::UserSttEnd => JsSpeechEventType::UserSttEnd,
            SpeechEventKind::UserSttNotFound => JsSpeechEventType::UserSttNotFound,
            SpeechEventKind::BargeIn => JsSpeechEventType::BargeIn,
            SpeechEventKind::Error => JsSpeechEventType::Error,
            SpeechEventKind::SttConfigUpdated => JsSpeechEventType::SttConfigUpdated,
            SpeechEventKind::TtsConfigUpdated => JsSpeechEventType::TtsConfigUpdated,
        },
        text: event.text,
        language: event.language,
        error: event.error,
        utterance_id: event.utterance_id,
        replay: event.replay,
        replaces_utterance_id: event.replaces_utterance_id,
        language_mismatch: event.language_mismatch,
        voice: event.voice,
        model_path: event.model_path,
        endpoint: event.endpoint,
    }
}

#[cfg(test)]
mod speech_event_to_js_tests {
    use super::*;
    use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventKind};

    #[test]
    fn user_language_maps_language_field() {
        let js = speech_event_to_js(SpeechEvent::user_language("fr", Some("utt-1".into())));
        assert_eq!(js.event_type, JsSpeechEventType::UserLanguage);
        assert_eq!(js.language.as_deref(), Some("fr"));
        assert_eq!(js.text.as_deref(), Some("fr"));
        assert_eq!(js.utterance_id.as_deref(), Some("utt-1"));
    }
}

pub fn wire_speech_callback(
    env: &Env,
    callback: JsFunction,
    mut rx: tokio::sync::broadcast::Receiver<SpeechEvent>,
) -> Result<()> {
    let tsfn = create_event_callback(
        env,
        callback,
        move |ctx: ThreadSafeCallContext<SpeechEvent>| Ok(vec![speech_event_to_js(ctx.value)]),
    )?;
    spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let status = tsfn.call(Ok(event), ThreadsafeFunctionCallMode::NonBlocking);
                    if status != napi::Status::Ok {
                        eprintln!("[voice-debug] speech callback: tsfn.call status={status:?}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    eprintln!("[voice-debug] speech callback: lagged, skipped {skipped} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Ok(())
}
