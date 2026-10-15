//! Full-stack: while utterance A waits 10 s for a slow cluster-STT final, the next utterance B
//! must still be heard on time (VAD events, barge-in), and both finals arrive in order with the
//! right utterance ids.
//!
//! The inbound path never waits for the STT vendor: results are produced by the STT actor and
//! emitted by the agent's result pump, so a slow final for A holds only A's terminal event.

mod fullstack_common;
mod mock_server;

use std::sync::atomic::Ordering;
use std::time::Duration;

use fullstack_common::{
    default_vad, first_at_or_after, position_of, silence_past_gate_hold_ms, speaking_start_ids,
    Harness, FRAME_MS,
};
use mock_server::MockSpeechState;
use node_webrtc_rust_speech::config::SendTextToTtsOptions;
use node_webrtc_rust_speech::events::SpeechEventKind;

const SPEECH_MS: u64 = 2_500;
const SLOW_FINAL_MS: usize = 10_000;
/// Finalize wait (`SPEECH_STT_FINALIZE_WAIT_MS`) longer than the vendor's 10 s delay, so A's real
/// final is delivered. With the 8 s default the agent closes A with a fallback final taken from
/// its last partial and ignores the late real one. Both tests of this binary set the same value.
const FINALIZE_WAIT_MS: &str = "15000";
/// What the mock answers to a finalized stream; the fallback final would read "partial".
const MOCK_FINAL_TEXT: &str = "final text";
/// A's real final may not arrive earlier than this after the end of A's speech.
const MIN_FINAL_AFTER_SPEECH_END_MS: u128 = 9_000;

fn set_finalize_wait_env() {
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("SPEECH_STT_FINALIZE_WAIT_MS", FINALIZE_WAIT_MS);
    }
}

/// A's `user_speech_final` carries the vendor's real final (not the partial fallback) and arrives
/// at least `MIN_FINAL_AFTER_SPEECH_END_MS` after A's speech ended; B's final is the real one too.
#[track_caller]
fn assert_real_late_final(
    events: &[fullstack_common::TaggedEvent],
    a_final_pos: usize,
    b_final_pos: usize,
    a_speech_end_ms: u128,
    timeline: &str,
) {
    let a_final = &events[a_final_pos];
    assert_eq!(
        a_final.text.as_deref(),
        Some(MOCK_FINAL_TEXT),
        "A's final is not the vendor's real final (fallback taken?)\n{timeline}"
    );
    assert!(
        a_final.at_ms >= a_speech_end_ms + MIN_FINAL_AFTER_SPEECH_END_MS,
        "A's final arrived {} ms after A's speech ended (expected >= {MIN_FINAL_AFTER_SPEECH_END_MS})\n{timeline}",
        a_final.at_ms.saturating_sub(a_speech_end_ms)
    );
    assert_eq!(
        events[b_final_pos].text.as_deref(),
        Some(MOCK_FINAL_TEXT),
        "B's final is not the vendor's real final\n{timeline}"
    );
}
const MAX_CALL_MS: u128 = 1_000;
/// Frames of slack between the first loud frame of an utterance and its VAD events.
const EVENT_SLACK_FRAMES: u64 = 10;

/// B's VAD events arrive within a few frames of B's first loud frame while A's final is 10 s late;
/// B's start precedes A's final; both finals carry their own id, A's first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vad_events_of_next_utterance_on_time_during_10s_slow_final() {
    set_finalize_wait_env();
    let vad = default_vad();
    let past_hold_ms = silence_past_gate_hold_ms(&vad);
    let mut h = Harness::start(MockSpeechState::default(), vad).await;

    // Turn A: 2.5 s of speech, its final takes 10 s.
    h.state
        .final_delay_ms
        .store(SLOW_FINAL_MS, Ordering::SeqCst);
    h.feed_ms(true, SPEECH_MS).await;
    let a_speech_end_ms = h.elapsed_ms();
    h.feed_ms(false, past_hold_ms).await;
    h.state.final_delay_ms.store(0, Ordering::SeqCst);

    // Turn B: 2.5 s of speech, then 6 s of silence.
    let b_first_loud = h.frame_index();
    h.feed_ms(true, SPEECH_MS).await;
    h.feed_ms(false, 6_000).await;

    let idle = h.agent.wait_stt_idle(Duration::from_secs(20)).await;
    let timeline = h.timeline();
    eprintln!("{timeline}");
    let (events, max_call_ms, p99_call_ms) = h.finish().await;
    eprintln!(
        "vad_events_of_next_utterance_on_time: max_call_ms={max_call_ms} p99_call_ms={p99_call_ms}"
    );

    assert!(idle, "wait_stt_idle(20 s) timed out\n{timeline}");
    let ids = speaking_start_ids(&events);
    assert_eq!(ids.len(), 2, "expected two utterances\n{timeline}");
    let (a_id, b_id) = (&ids[0], &ids[1]);

    let b_vad = first_at_or_after(&events, SpeechEventKind::VadTriggered, b_first_loud)
        .unwrap_or_else(|| panic!("no vad_triggered for B\n{timeline}"));
    assert!(
        b_vad.frame <= b_first_loud + EVENT_SLACK_FRAMES,
        "B vad_triggered tagged frame {} (B first loud frame {b_first_loud})\n{timeline}",
        b_vad.frame
    );
    let b_start_pos = position_of(&events, SpeechEventKind::UserSpeakingStart, b_id).unwrap();
    assert!(
        events[b_start_pos].frame <= b_first_loud + EVENT_SLACK_FRAMES,
        "B user_speaking_start tagged frame {} (B first loud frame {b_first_loud})\n{timeline}",
        events[b_start_pos].frame
    );

    let a_final_pos = position_of(&events, SpeechEventKind::UserSpeechFinal, a_id)
        .unwrap_or_else(|| panic!("no user_speech_final for A\n{timeline}"));
    let b_final_pos = position_of(&events, SpeechEventKind::UserSpeechFinal, b_id)
        .unwrap_or_else(|| panic!("no user_speech_final for B\n{timeline}"));
    assert!(
        b_start_pos < a_final_pos,
        "B's user_speaking_start did not precede A's user_speech_final (no overlap)\n{timeline}"
    );
    assert!(
        a_final_pos < b_final_pos,
        "A's terminal did not precede B's\n{timeline}"
    );
    assert_real_late_final(
        &events,
        a_final_pos,
        b_final_pos,
        a_speech_end_ms,
        &timeline,
    );
    assert!(
        !events
            .iter()
            .any(|e| e.kind == SpeechEventKind::UserSttNotFound),
        "unexpected user_stt_not_found\n{timeline}"
    );
    assert!(
        max_call_ms < MAX_CALL_MS,
        "max process_inbound_pcm call {max_call_ms} ms\n{timeline}"
    );
}

/// Agent TTS is playing while A's final is 10 s late; B's speech barges in on time and flushes the
/// playback immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn barge_in_on_time_during_10s_slow_final() {
    set_finalize_wait_env();
    let mut vad = default_vad();
    vad.barge_in.enabled = true;
    vad.barge_in.use_vad = true;
    vad.barge_in.require_stt_partial = false;
    let past_hold_ms = silence_past_gate_hold_ms(&vad);
    // About 20 s of reply audio: long enough that it is still playing when B speaks.
    let state = MockSpeechState::default();
    state
        .synthesize_single_message_bytes
        .store(1_000_000, Ordering::SeqCst);
    let mut h = Harness::start(state, vad).await;

    // Turn A: 2.5 s of speech, its final takes 10 s.
    h.state
        .final_delay_ms
        .store(SLOW_FINAL_MS, Ordering::SeqCst);
    h.feed_ms(true, SPEECH_MS).await;
    let a_speech_end_ms = h.elapsed_ms();
    h.feed_ms(false, past_hold_ms).await;
    h.state.final_delay_ms.store(0, Ordering::SeqCst);

    // The agent starts a reply while A's final is still pending.
    h.agent
        .send_text_to_tts_with_options(
            "one moment please, I am still checking that for you, this takes a few seconds",
            SendTextToTtsOptions {
                non_blocking: true,
                ..SendTextToTtsOptions::default()
            },
        )
        .await
        .unwrap();
    let mut speaking = false;
    for _ in 0..(5_000 / FRAME_MS) {
        if h.events()
            .iter()
            .any(|e| e.kind == SpeechEventKind::AgentSpeakingStart)
        {
            speaking = true;
            break;
        }
        h.feed(false).await;
    }
    assert!(
        speaking,
        "no agent_speaking_start within 5 s\n{}",
        h.timeline()
    );

    // Turn B barges in.
    let b_first_loud = h.frame_index();
    h.feed_ms(true, SPEECH_MS).await;
    h.feed_ms(false, 6_000).await;

    let idle = h.agent.wait_stt_idle(Duration::from_secs(20)).await;
    let timeline = h.timeline();
    eprintln!("{timeline}");
    let writes_total = h.writer_calls();
    let (events, max_call_ms, p99_call_ms) = h.finish().await;
    eprintln!("barge_in_on_time: max_call_ms={max_call_ms} p99_call_ms={p99_call_ms}");

    assert!(idle, "wait_stt_idle(20 s) timed out\n{timeline}");
    let ids = speaking_start_ids(&events);
    assert_eq!(ids.len(), 2, "expected two utterances\n{timeline}");
    let (a_id, b_id) = (&ids[0], &ids[1]);

    let barge_pos = events
        .iter()
        .position(|e| e.kind == SpeechEventKind::BargeIn)
        .unwrap_or_else(|| panic!("no barge_in\n{timeline}"));
    let barge = &events[barge_pos];
    assert!(
        barge.frame <= b_first_loud + EVENT_SLACK_FRAMES,
        "barge_in tagged frame {} (B first loud frame {b_first_loud})\n{timeline}",
        barge.frame
    );
    let end = events[barge_pos..]
        .iter()
        .find(|e| e.kind == SpeechEventKind::AgentSpeakingEnd)
        .unwrap_or_else(|| panic!("no agent_speaking_end after barge_in\n{timeline}"));
    assert!(
        end.frame <= barge.frame + 1,
        "agent_speaking_end tagged frame {} (barge_in frame {})\n{timeline}",
        end.frame,
        barge.frame
    );
    assert!(
        writes_total <= barge.writer_calls + 2,
        "PcmWriter calls grew from {} at barge_in to {writes_total}\n{timeline}",
        barge.writer_calls
    );

    let a_final_pos = position_of(&events, SpeechEventKind::UserSpeechFinal, a_id)
        .unwrap_or_else(|| panic!("no user_speech_final for A\n{timeline}"));
    let b_final_pos = position_of(&events, SpeechEventKind::UserSpeechFinal, b_id)
        .unwrap_or_else(|| panic!("no user_speech_final for B\n{timeline}"));
    assert!(
        barge_pos < a_final_pos,
        "barge_in did not precede A's user_speech_final\n{timeline}"
    );
    assert!(
        a_final_pos < b_final_pos,
        "A's terminal did not precede B's\n{timeline}"
    );
    assert_real_late_final(
        &events,
        a_final_pos,
        b_final_pos,
        a_speech_end_ms,
        &timeline,
    );
    assert!(
        max_call_ms < MAX_CALL_MS,
        "max process_inbound_pcm call {max_call_ms} ms\n{timeline}"
    );
}
