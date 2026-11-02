//! `send_text_to_tts*` queues jobs in call order (ticket order), not in the order the send
//! futures happen to reach the queue.
//!
//! Earlier sends are held back before the queue by the pre-enqueue test hook (one gate per
//! ticket, released by the test in reverse order), so a later send always arrives first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use node_webrtc_rust_speech::agent::TtsPreEnqueueHook;
use node_webrtc_rust_speech::config::{
    SendTextToTtsOptions, SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

const N: u64 = 24;
const BOUND: Duration = Duration::from_secs(20);

fn config(with_tts: bool) -> VoiceAgentConfig {
    VoiceAgentConfig {
        stt: None,
        tts: with_tts.then(|| TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        }),
        vad: VadConfig {
            enabled: false,
            ..VadConfig::default()
        },
        ..Default::default()
    }
}

async fn make_agent(with_tts: bool) -> Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));
    let agent = VoiceAgent::new(config(with_tts), Arc::new(registry)).unwrap();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    agent
}

type Gates = Arc<Mutex<HashMap<u64, oneshot::Receiver<()>>>>;

/// Installs a hook that parks ticket `t` until its gate sender is fired (or dropped).
fn install_gates(
    agent: &VoiceAgent,
    tickets: impl Iterator<Item = u64>,
) -> HashMap<u64, oneshot::Sender<()>> {
    let gates: Gates = Arc::new(Mutex::new(HashMap::new()));
    let mut senders = HashMap::new();
    for t in tickets {
        let (tx, rx) = oneshot::channel();
        gates.lock().unwrap().insert(t, rx);
        senders.insert(t, tx);
    }
    let hook: TtsPreEnqueueHook = Arc::new(move |ticket| {
        let rx = gates.lock().unwrap().remove(&ticket);
        Box::pin(async move {
            if let Some(rx) = rx {
                let _ = rx.await;
            }
        })
    });
    agent.set_tts_pre_enqueue_hook(Some(hook));
    senders
}

fn tagged(id: &str) -> SendTextToTtsOptions {
    SendTextToTtsOptions {
        non_blocking: true,
        reply_id: Some(id.to_string()),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sends_are_queued_and_played_in_call_order() {
    let agent = make_agent(true).await;
    let mut rx = agent.subscribe_events();
    let mut gates = install_gates(&agent, 0..N);

    // Tickets are taken here, synchronously, in call order.
    let mut handles = Vec::new();
    for i in 0..N {
        let ticket = agent.reserve_tts_send();
        assert_eq!(ticket.id(), i);
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("{i}"), tagged(&format!("r{i}")))
                .await
        }));
    }
    // The last call reaches the queue first, the first call last.
    for i in (0..N).rev() {
        gates.remove(&i).unwrap().send(()).unwrap();
        tokio::task::yield_now().await;
    }
    for h in handles {
        timeout(BOUND, h)
            .await
            .expect("send bounded")
            .unwrap()
            .unwrap();
    }

    let mut starts = Vec::new();
    timeout(BOUND, async {
        while starts.len() < N as usize {
            let event = rx.recv().await.unwrap();
            if event.kind == SpeechEventKind::AgentReplyStart {
                starts.push(event.reply_id.unwrap_or_default());
            }
        }
    })
    .await
    .expect("all replies start");
    let expected: Vec<String> = (0..N).map(|i| format!("r{i}")).collect();
    assert_eq!(
        starts, expected,
        "agent_reply_start order must equal call order"
    );
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_holds_jobs_in_ticket_order() {
    let agent = make_agent(true).await;
    // Keep the worker from dequeuing: hold the first ticket's job back, inspect the rest.
    let mut gates = install_gates(&agent, 0..N);
    let mut handles = Vec::new();
    for i in 0..N {
        let ticket = agent.reserve_tts_send();
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("q {i}"), tagged(&format!("q{i}")))
                .await
        }));
    }
    for i in (1..N).rev() {
        gates.remove(&i).unwrap().send(()).unwrap();
    }
    // Tickets 1.. are past their gate but must not be queued ahead of ticket 0.
    tokio::task::yield_now().await;
    assert!(
        agent.queued_tts_tickets().await.is_empty(),
        "no job may jump ahead of ticket 0"
    );
    gates.remove(&0).unwrap().send(()).unwrap();
    for h in handles {
        timeout(BOUND, h)
            .await
            .expect("send bounded")
            .unwrap()
            .unwrap();
    }
    let queued = agent.queued_tts_tickets().await;
    let mut sorted = queued.clone();
    sorted.sort_unstable();
    assert_eq!(queued, sorted, "queue must stay in ticket order");
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_that_gives_up_does_not_block_later_tickets() {
    // Error path: no TTS configured, every send errors and must release its turn.
    let agent = make_agent(false).await;
    let mut handles = Vec::new();
    for i in 0..N {
        let ticket = agent.reserve_tts_send();
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("x {i}"), tagged("x"))
                .await
        }));
    }
    for h in handles {
        let res = timeout(BOUND, h)
            .await
            .expect("error path bounded")
            .unwrap();
        assert!(res.unwrap_err().to_string().contains("TTS not configured"));
    }

    // Aborted send future and an unused (dropped) ticket.
    let agent = make_agent(true).await;
    let _gates = install_gates(&agent, 0..1); // ticket 0 parks forever
    let t0 = agent.reserve_tts_send();
    let t1 = agent.reserve_tts_send();
    let t2 = agent.reserve_tts_send();
    let a0 = Arc::clone(&agent);
    let parked = tokio::spawn(async move {
        a0.send_text_to_tts_reserved(t0, "parked", tagged("p0"))
            .await
    });
    tokio::task::yield_now().await;
    parked.abort();
    drop(t1);
    timeout(
        BOUND,
        agent.send_text_to_tts_reserved(t2, "after", tagged("p2")),
    )
    .await
    .expect("ticket 2 is not blocked by an aborted or dropped lower ticket")
    .unwrap();
    agent.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_cancels_tickets_still_on_their_way_and_keeps_later_ones() {
    let agent = make_agent(true).await;
    let mut gates = install_gates(&agent, 0..6);
    let mut handles = Vec::new();
    for i in 0..6u64 {
        let ticket = agent.reserve_tts_send();
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent
                .send_text_to_tts_reserved(ticket, &format!("f {i}"), tagged(&format!("f{i}")))
                .await
        }));
    }
    let cut = agent.begin_flush_tts();
    let later = agent.reserve_tts_send();
    timeout(BOUND, agent.flush_tts_from(cut))
        .await
        .expect("flush bounded")
        .unwrap();

    // The later send is not blocked by the cancelled, still-parked tickets 0..6.
    timeout(
        BOUND,
        agent.send_text_to_tts_reserved(later, "kept", tagged("k")),
    )
    .await
    .expect("later ticket not blocked by cancelled ones")
    .unwrap();

    for i in 0..6u64 {
        gates.remove(&i).unwrap().send(()).unwrap();
    }
    for h in handles {
        let res = timeout(BOUND, h)
            .await
            .expect("cancelled send bounded")
            .unwrap();
        assert!(
            res.unwrap_err().to_string().contains("TTS cancelled"),
            "a send called before flush is cancelled"
        );
    }
    agent.stop().await.unwrap();
}
