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
            SpeechEventKind::UserSpeechIgnored => JsSpeechEventType::UserSpeechIgnored,
            SpeechEventKind::UserLanguage => JsSpeechEventType::UserLanguage,
            SpeechEventKind::LanguageIdSkipped => JsSpeechEventType::LanguageIdSkipped,
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
            SpeechEventKind::SttHoldStarted => JsSpeechEventType::SttHoldStarted,
            SpeechEventKind::SttHoldEnded => JsSpeechEventType::SttHoldEnded,
            SpeechEventKind::TtsWait => JsSpeechEventType::TtsWait,
            SpeechEventKind::TtsPhraseCacheHit => JsSpeechEventType::TtsPhraseCacheHit,
            SpeechEventKind::AgentSpeakFailed => JsSpeechEventType::AgentSpeakFailed,
            SpeechEventKind::AgentReplyStart => JsSpeechEventType::AgentReplyStart,
            SpeechEventKind::AgentReplyEnd => JsSpeechEventType::AgentReplyEnd,
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
        hold_mode: event.hold_mode,
        hold_outcome: event.hold_outcome,
        buffered_ms: event.buffered_ms,
        dropped_ms: event.dropped_ms,
        reason: event.reason,
        speech_ms: event.speech_ms,
        first_chunk_ms: event.first_chunk_ms,
        first_audio_ms: event.first_audio_ms,
        sentence_case_adjusted: event.sentence_case_adjusted,
        wait_ms: event.wait_ms,
        attempts: event.attempts,
        chars: event.chars,
        reply_id: event.reply_id,
        played_ms: event.played_ms,
        synthesized_ms: event.synthesized_ms,
        synthesis_complete: event.synthesis_complete,
        interrupted: event.interrupted,
        speaker: event.speaker,
        speaker_unknown_reason: event.speaker_unknown_reason,
        speaker_rechecked: event.speaker_rechecked,
        speaker_check_windows: event.speaker_check_windows,
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

#[cfg(test)]
mod speech_event_to_js_tests {
    use super::*;
    use node_webrtc_rust_speech::events::SpeechEvent;

    #[test]
    fn user_language_maps_language_field() {
        let js = speech_event_to_js(SpeechEvent::user_language("fr", Some("utt-1".into())));
        assert_eq!(js.event_type, JsSpeechEventType::UserLanguage);
        assert_eq!(js.language.as_deref(), Some("fr"));
        assert_eq!(js.text.as_deref(), Some("fr"));
        assert_eq!(js.utterance_id.as_deref(), Some("utt-1"));
    }

    #[test]
    fn language_id_skipped_maps_reason_and_speech_ms() {
        let js = speech_event_to_js(SpeechEvent::language_id_skipped(
            "deferred_tts",
            80,
            Some("utt-2".into()),
        ));
        assert_eq!(js.event_type, JsSpeechEventType::LanguageIdSkipped);
        assert_eq!(js.reason.as_deref(), Some("deferred_tts"));
        assert_eq!(js.speech_ms, Some(80));
        assert!(js.text.is_none());
    }

    #[test]
    fn speaker_and_ignored_map_fields() {
        let js = speech_event_to_js(
            SpeechEvent::user_speech_final("hi", Some("u1".into())).with_speaker("other"),
        );
        assert_eq!(js.speaker.as_deref(), Some("other"));
        assert!(js.speaker_unknown_reason.is_none());
        let js = speech_event_to_js(
            SpeechEvent::user_speech_final("hi", Some("u1".into()))
                .with_speaker("unknown")
                .with_speaker_unknown_reason("short"),
        );
        assert_eq!(js.speaker_unknown_reason.as_deref(), Some("short"));
        assert!(js.speaker_rechecked.is_none());
        let js = speech_event_to_js(
            SpeechEvent::user_speech_final("hi", Some("u1".into()))
                .with_speaker("caller")
                .with_speaker_rechecked(),
        );
        assert_eq!(js.speaker_rechecked, Some(true));
        assert!(js.speaker_check_windows.is_none());
        let js = speech_event_to_js(
            SpeechEvent::user_speech_final("hi", Some("u1".into()))
                .with_speaker("caller")
                .with_speaker_check_windows(3),
        );
        assert_eq!(js.speaker_check_windows, Some(3));
        let js = speech_event_to_js(SpeechEvent::user_speech_ignored(Some("u2".into())));
        assert_eq!(js.event_type, JsSpeechEventType::UserSpeechIgnored);
        assert_eq!(js.reason.as_deref(), Some("speaker_mismatch"));
        assert!(js.text.is_none());
    }

    #[test]
    fn stt_stream_end_maps_reason() {
        let js = speech_event_to_js(SpeechEvent::stt_stream_end_with_reason("end_of_utterance"));
        assert_eq!(js.event_type, JsSpeechEventType::SttStreamEnd);
        assert_eq!(js.reason.as_deref(), Some("end_of_utterance"));
    }

    #[test]
    fn tts_wait_and_speak_failed_map_fields() {
        let js = speech_event_to_js(SpeechEvent::tts_wait(1500, 7, "Unavailable loading"));
        assert_eq!(js.event_type, JsSpeechEventType::TtsWait);
        assert_eq!(js.wait_ms, Some(1500));
        assert_eq!(js.attempts, Some(7));
        assert_eq!(js.reason.as_deref(), Some("Unavailable loading"));

        let js = speech_event_to_js(SpeechEvent::agent_speak_failed("boom"));
        assert_eq!(js.event_type, JsSpeechEventType::AgentSpeakFailed);
        assert_eq!(js.reason.as_deref(), Some("boom"));
    }

    #[test]
    fn tts_phrase_cache_hit_maps_chars() {
        let js = speech_event_to_js(SpeechEvent::tts_phrase_cache_hit(12));
        assert_eq!(js.event_type, JsSpeechEventType::TtsPhraseCacheHit);
        assert_eq!(js.chars, Some(12));
        assert!(js.text.is_none());
    }

    #[test]
    fn reply_events_map_reply_id_and_playback() {
        let js = speech_event_to_js(SpeechEvent::agent_reply_start("r1", Some(120)));
        assert_eq!(js.event_type, JsSpeechEventType::AgentReplyStart);
        assert_eq!(js.reply_id.as_deref(), Some("r1"));
        assert_eq!(js.first_chunk_ms, Some(120));

        let js = speech_event_to_js(SpeechEvent::agent_reply_end("r1", 900, true, 1500, false));
        assert_eq!(js.event_type, JsSpeechEventType::AgentReplyEnd);
        assert_eq!(js.played_ms, Some(900));
        assert_eq!(js.interrupted, Some(true));
        assert_eq!(js.synthesized_ms, Some(1500));
        assert_eq!(js.synthesis_complete, Some(false));

        let js = speech_event_to_js(SpeechEvent::tts_phrase_cache_hit(3).with_reply_id(Some("r2")));
        assert_eq!(js.reply_id.as_deref(), Some("r2"));
    }

    #[test]
    fn agent_speaking_start_maps_first_audio_timing() {
        let js = speech_event_to_js(SpeechEvent::agent_speaking_start_with_timing(
            Some(240),
            Some(180),
            Some(2),
        ));
        assert_eq!(js.event_type, JsSpeechEventType::AgentSpeakingStart);
        assert_eq!(js.first_audio_ms, Some(240));
        assert_eq!(js.first_chunk_ms, Some(180));
        assert_eq!(js.sentence_case_adjusted, Some(2));

        let plain = speech_event_to_js(SpeechEvent::agent_speaking_start());
        assert_eq!(plain.first_audio_ms, None);
        assert_eq!(plain.first_chunk_ms, None);
        assert_eq!(plain.sentence_case_adjusted, None);
    }
}
