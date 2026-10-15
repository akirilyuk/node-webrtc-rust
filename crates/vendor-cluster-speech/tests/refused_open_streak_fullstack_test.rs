//! Full-stack: a ~60 s streak of refused Transcribe opens (speech pod full, `RESOURCE_EXHAUSTED`,
//! retried every 200 ms) must not slow the inbound path or lose audio. Speech events keep
//! flowing on time, and once the streak ends every queued utterance is finalized in order.

mod fullstack_common;
mod mock_server;

use std::sync::atomic::Ordering;
use std::time::Duration;

use fullstack_common::{
    default_vad, first_at_or_after, is_terminal, speaking_start_ids, Harness, TaggedEvent,
};
use mock_server::MockSpeechState;
use node_webrtc_rust_speech::events::SpeechEventKind;

const SPEECH_MS: u64 = 2_000;
const SILENCE_MS: u64 = 3_000;
const MAX_CALL_MS: u128 = 1_000;
const EVENT_SLACK_FRAMES: u64 = 10;
/// Bound on the whole run.
const RUN_BOUND_MS: u128 = 150_000;
/// ~60 s of 200 ms open retries.
const REFUSED_OPENS: usize = 300;

fn terminal_ids(events: &[TaggedEvent]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for e in events.iter().filter(|e| is_terminal(e.kind)) {
        if let Some(id) = &e.utterance_id {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixty_second_refused_open_streak_keeps_events_flowing() {
    let mut h = Harness::start(MockSpeechState::default(), default_vad()).await;

    // Turn 1: normal, gets its final.
    let mut first_loud = vec![h.frame_index()];
    h.feed_ms(true, SPEECH_MS).await;
    for _ in 0..(10_000 / 20) {
        if !terminal_ids(&h.events()).is_empty() {
            break;
        }
        h.feed(false).await;
    }
    assert!(
        !terminal_ids(&h.events()).is_empty(),
        "turn 1 got no terminal event\n{}",
        h.timeline()
    );

    // The streak starts just before turn 2: its stream open is refused ~300 times.
    h.state
        .reject_first_opens
        .store(REFUSED_OPENS, Ordering::SeqCst);
    for _ in 0..3 {
        first_loud.push(h.frame_index());
        h.feed_ms(true, SPEECH_MS).await;
        h.feed_ms(false, SILENCE_MS).await;
    }

    // Keep the cadence going until every turn has a terminal event.
    while terminal_ids(&h.events()).len() < 4 && h.elapsed_ms() < RUN_BOUND_MS {
        h.feed(false).await;
    }

    let idle = h.agent.wait_stt_idle(Duration::from_secs(20)).await;
    let bytes_sent = h.agent.stt_audio_bytes_sent();
    let bytes_received = h.state.audio_bytes_received.load(Ordering::SeqCst);
    let rejected = h.state.rejected_opens.load(Ordering::SeqCst);
    let timeline = h.timeline();
    eprintln!("{timeline}");
    eprintln!("rejected_opens={rejected} audio_bytes_received={bytes_received} stt_audio_bytes_sent={bytes_sent}");
    let (events, max_call_ms, p99_call_ms) = h.finish().await;
    eprintln!("refused_open_streak: max_call_ms={max_call_ms} p99_call_ms={p99_call_ms}");

    assert!(idle, "wait_stt_idle(20 s) timed out\n{timeline}");
    let ids = speaking_start_ids(&events);
    assert_eq!(ids.len(), 4, "expected four utterances\n{timeline}");

    for (turn, id) in ids.iter().enumerate() {
        let loud = first_loud[turn];
        let vad = first_at_or_after(&events, SpeechEventKind::VadTriggered, loud)
            .unwrap_or_else(|| panic!("turn {}: no vad_triggered\n{timeline}", turn + 1));
        assert!(
            vad.frame <= loud + EVENT_SLACK_FRAMES,
            "turn {}: vad_triggered tagged frame {} (first loud frame {loud})\n{timeline}",
            turn + 1,
            vad.frame
        );
        let start = events
            .iter()
            .find(|e| {
                e.kind == SpeechEventKind::UserSpeakingStart
                    && e.utterance_id.as_deref() == Some(id)
            })
            .unwrap();
        assert!(
            start.frame <= loud + EVENT_SLACK_FRAMES,
            "turn {}: user_speaking_start tagged frame {} (first loud frame {loud})\n{timeline}",
            turn + 1,
            start.frame
        );

        let terminals: Vec<&TaggedEvent> = events
            .iter()
            .filter(|e| is_terminal(e.kind) && e.utterance_id.as_deref() == Some(id))
            .collect();
        assert_eq!(
            terminals.len(),
            1,
            "turn {}: expected exactly one terminal event\n{timeline}",
            turn + 1
        );
        if turn >= 1 {
            assert!(
                terminals[0].kind == SpeechEventKind::UserSpeechFinal
                    && terminals[0]
                        .text
                        .as_deref()
                        .is_some_and(|t| !t.trim().is_empty()),
                "turn {}: expected a non-empty user_speech_final, got {:?} {:?}\n{timeline}",
                turn + 1,
                terminals[0].kind,
                terminals[0].text
            );
        }
    }

    // Terminals are in turn order.
    assert_eq!(
        terminal_ids(&events),
        ids,
        "terminal events not in utterance order\n{timeline}"
    );

    assert!(
        max_call_ms < MAX_CALL_MS,
        "max process_inbound_pcm call {max_call_ms} ms\n{timeline}"
    );
    assert!(
        rejected >= 250,
        "only {rejected} opens were refused (expected the streak to last ~300)\n{timeline}"
    );
    assert_eq!(
        bytes_received as u64, bytes_sent,
        "audio lost: mock received {bytes_received} bytes, actor sent {bytes_sent}\n{timeline}"
    );
}
