//! Guard for the helpers-integration DTLS stall (node-webrtc-rust vendored patch, CI run
//! 43696405641): `dial()` / `accept()` returned as soon as the pair was selected, while the
//! agent's notifier task had received `Checking` but not yet called the state-change handler.
//! webrtc's `RTCIceTransport` mirrors the ICE state only inside that handler, so
//! `start_transports` -> `dtls_transport.start` read `New`, failed with "ICE connection not
//! started", and DTLS never started (peer stuck in `connecting`).
//!
//! The test hook `state_notifier_gate` holds the notifier between `recv()` and the handler call.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use util::sync::Mutex as SyncMutex;

use super::agent_vnet_test::gather_and_exchange_candidates;
use super::*;

type Seen = Arc<SyncMutex<Vec<ConnectionState>>>;

async fn new_agent(gate: &Arc<Semaphore>) -> Result<(Arc<Agent>, Seen)> {
    let _ = env_logger::builder().is_test(true).try_init();
    let cfg = AgentConfig {
        urls: vec![],
        // Loopback only: host LAN interfaces made connectivity depend on the machine (a macOS host
        // dropped every LAN STUN send, so dial() never selected a pair, gate or no gate).
        network_types: vec![NetworkType::Udp4],
        include_loopback: true,
        ip_filter: Arc::new(Some(Box::new(|ip: std::net::IpAddr| ip.is_loopback()))),
        ..Default::default()
    };
    let agent = Arc::new(Agent::new(cfg).await?);
    agent
        .internal
        .state_notifier_gate
        .store(Some(Arc::clone(gate)));
    let seen: Seen = Arc::new(SyncMutex::new(vec![]));
    let seen2 = Arc::clone(&seen);
    // Records at call time, like webrtc's RTCIceTransport handler stores its state at call time.
    agent.on_connection_state_change(Box::new(move |s: ConnectionState| {
        seen2.lock().push(s);
        Box::pin(async {})
    }));
    Ok((agent, seen))
}

/// Bounds for each phase of a test connect, so a stall names its phase instead of hanging.
const GATHER_BOUND: Duration = Duration::from_secs(10);
const CONNECT_BOUND: Duration = Duration::from_secs(10);
const RELEASER_BOUND: Duration = Duration::from_secs(10);
const TEST_BOUND: Duration = Duration::from_secs(30);

async fn phase<T>(name: &str, bound: Duration, fut: impl std::future::Future<Output = T>) -> T {
    match tokio::time::timeout(bound, fut).await {
        Ok(v) => v,
        Err(_) => panic!("phase {name} stalled for {bound:?}"),
    }
}

/// Connects a (accept) and b (dial) and returns, for each side, the states its handler had seen
/// at the moment `accept()` / `dial()` returned, and how long the call took.
async fn connect_with_gate(
    a: &Arc<Agent>,
    a_seen: &Seen,
    b: &Arc<Agent>,
    b_seen: &Seen,
) -> Result<(
    (Vec<ConnectionState>, Duration),
    (Vec<ConnectionState>, Duration),
)> {
    let (a_ufrag, a_pwd) = a.get_local_user_credentials().await;
    let (b_ufrag, b_pwd) = b.get_local_user_credentials().await;
    phase("gather", GATHER_BOUND, gather_and_exchange_candidates(a, b)).await?;

    let (_a_cancel_tx, a_cancel_rx) = mpsc::channel(1);
    let a2 = Arc::clone(a);
    let a_seen2 = Arc::clone(a_seen);
    let accept = tokio::spawn(async move {
        let t0 = Instant::now();
        let _conn = a2.accept(a_cancel_rx, b_ufrag, b_pwd).await?;
        let snapshot = a_seen2.lock().clone();
        Result::<_>::Ok((snapshot, t0.elapsed()))
    });

    let (_b_cancel_tx, b_cancel_rx) = mpsc::channel(1);
    let t0 = Instant::now();
    let _conn = phase("dial", CONNECT_BOUND, b.dial(b_cancel_rx, a_ufrag, a_pwd)).await?;
    let b_out = (b_seen.lock().clone(), t0.elapsed());
    let a_out = phase("accept", CONNECT_BOUND, accept)
        .await
        .map_err(|e| Error::Other(format!("accept task: {e}")))??;
    Ok((a_out, b_out))
}

async fn wait_sent(agent: &Arc<Agent>, n: u64) {
    while agent.internal.states_sent.load(Ordering::SeqCst) < n {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test]
async fn dial_and_accept_return_after_handler_saw_connected() -> Result<()> {
    let gate = Arc::new(Semaphore::new(0));
    let (a, a_seen) = new_agent(&gate).await?;
    let (b, b_seen) = new_agent(&gate).await?;

    // Release the notifiers only well after both agents queued Checking and Connected, so a
    // dial()/accept() that does not wait for delivery has already returned by then.
    let (a2, b2, gate2) = (Arc::clone(&a), Arc::clone(&b), Arc::clone(&gate));
    let releaser = tokio::spawn(async move {
        phase(
            "releaser wait accept-side sent",
            RELEASER_BOUND,
            wait_sent(&a2, 2),
        )
        .await;
        phase(
            "releaser wait dial-side sent",
            RELEASER_BOUND,
            wait_sent(&b2, 2),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        gate2.add_permits(1000);
    });

    let ((a_states, _), (b_states, _)) = phase(
        "total",
        TEST_BOUND,
        connect_with_gate(&a, &a_seen, &b, &b_seen),
    )
    .await?;
    phase("releaser", RELEASER_BOUND, releaser)
        .await
        .map_err(|e| Error::Other(format!("releaser task: {e}")))?;

    for (side, states) in [("accept", &a_states), ("dial", &b_states)] {
        assert!(
            states.contains(&ConnectionState::Checking)
                && states.contains(&ConnectionState::Connected),
            "{side}() returned before the state handler saw Checking and Connected: {states:?}"
        );
    }

    phase("close", CONNECT_BOUND, a.close()).await?;
    phase("close", CONNECT_BOUND, b.close()).await?;
    Ok(())
}

#[tokio::test]
async fn dial_and_accept_still_return_when_handler_never_runs() -> Result<()> {
    // The notifier never gets a permit: the wait is bounded and the connect still succeeds.
    let gate = Arc::new(Semaphore::new(0));
    let (a, a_seen) = new_agent(&gate).await?;
    let (b, b_seen) = new_agent(&gate).await?;

    let ((_, a_took), (_, b_took)) = phase(
        "total",
        TEST_BOUND,
        connect_with_gate(&a, &a_seen, &b, &b_seen),
    )
    .await?;
    for (side, took) in [("accept", a_took), ("dial", b_took)] {
        assert!(
            took >= STATE_DELIVERY_TIMEOUT && took < STATE_DELIVERY_TIMEOUT + Duration::from_secs(3),
            "{side}() took {took:?}, expected about STATE_DELIVERY_TIMEOUT ({STATE_DELIVERY_TIMEOUT:?})"
        );
    }

    gate.add_permits(1000);
    phase("close", CONNECT_BOUND, a.close()).await?;
    phase("close", CONNECT_BOUND, b.close()).await?;
    Ok(())
}

#[tokio::test]
async fn dial_cancelled_while_waiting_for_state_delivery_returns_cancel_error() -> Result<()> {
    // The notifier never gets a permit, so dial() sits in the state-delivery wait; a cancel there
    // returns ErrCanceledByCaller at once, as a cancel before the pair was selected does.
    let gate = Arc::new(Semaphore::new(0));
    let (a, _a_seen) = new_agent(&gate).await?;
    let (b, _b_seen) = new_agent(&gate).await?;
    let (a_ufrag, a_pwd) = a.get_local_user_credentials().await;
    let (b_ufrag, b_pwd) = b.get_local_user_credentials().await;
    phase(
        "gather",
        GATHER_BOUND,
        gather_and_exchange_candidates(&a, &b),
    )
    .await?;

    let (_a_cancel_tx, a_cancel_rx) = mpsc::channel(1);
    let a2 = Arc::clone(&a);
    let accept =
        tokio::spawn(async move { a2.accept(a_cancel_rx, b_ufrag, b_pwd).await.map(|_| ()) });

    let (b_cancel_tx, b_cancel_rx) = mpsc::channel(1);
    let b2 = Arc::clone(&b);
    let dial = tokio::spawn(async move { b2.dial(b_cancel_rx, a_ufrag, a_pwd).await.map(|_| ()) });

    // Connected is queued just before on_connected fires; give dial() time to enter the wait.
    phase("wait dial-side sent", CONNECT_BOUND, wait_sent(&b, 2)).await;
    phase("wait dial-side pair selected", CONNECT_BOUND, async {
        while b.internal.agent_conn.selected_pair.load().is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !dial.is_finished(),
        "dial() returned before the cancel, with the handler held"
    );

    let t0 = Instant::now();
    b_cancel_tx.send(()).await.expect("send cancel");
    let res = phase("cancelled dial", CONNECT_BOUND, dial)
        .await
        .map_err(|e| Error::Other(format!("dial task: {e}")))?;
    let took = t0.elapsed();
    assert_eq!(
        res,
        Err(Error::ErrCanceledByCaller),
        "cancelled dial() result"
    );
    assert!(
        took < Duration::from_millis(500),
        "cancelled dial() took {took:?} to return after the cancel"
    );

    gate.add_permits(1000);
    phase("accept", CONNECT_BOUND, accept)
        .await
        .map_err(|e| Error::Other(format!("accept task: {e}")))??;
    phase("close", CONNECT_BOUND, a.close()).await?;
    phase("close", CONNECT_BOUND, b.close()).await?;
    Ok(())
}
