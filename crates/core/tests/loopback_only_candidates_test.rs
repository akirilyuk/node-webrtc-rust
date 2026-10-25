//! `WEBRTC_LOOPBACK_ONLY_CANDIDATES=1` gathers loopback ICE candidates only.
//!
//! Own test binary with a single test: the setting engine is built once per process, so the env
//! var must be set before the first `PeerConnection::new`, and no other test may race the setenv.

use std::time::Duration;

use node_webrtc_rust_core::{ConnectionState, PeerConnection, PeerConnectionConfig};
use tokio::time::{sleep, timeout};

fn candidate_addresses(sdp: &str) -> Vec<String> {
    sdp.lines()
        .filter_map(|l| l.trim().strip_prefix("a=candidate:"))
        .map(|rest| rest.split_whitespace().nth(4).unwrap_or("").to_owned())
        .collect()
}

async fn wait_connected(pc: &PeerConnection) {
    timeout(Duration::from_secs(25), async {
        loop {
            match pc.connection_state() {
                ConnectionState::Connected => return,
                ConnectionState::Failed | ConnectionState::Closed => {
                    panic!("connection failed: {:?}", pc.connection_state())
                }
                _ => sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .expect("timed out waiting for connection");
}

#[tokio::test]
async fn loopback_only_gathers_only_loopback_and_connects() {
    // SAFETY: the only test in this binary, set before any thread reads the environment.
    unsafe { std::env::set_var("WEBRTC_LOOPBACK_ONLY_CANDIDATES", "1") };

    let config = PeerConnectionConfig {
        ice_servers: vec![],
        ..Default::default()
    };
    let offer = PeerConnection::new(config.clone()).await.expect("pc1");
    let answer = PeerConnection::new(config).await.expect("pc2");

    offer
        .create_data_channel("_signal", None)
        .await
        .expect("create channel");
    let desc = offer.create_offer(None).await.expect("create offer");
    offer.set_local_description(desc).await.expect("set local");
    timeout(Duration::from_secs(10), offer.gathering_complete())
        .await
        .expect("gathering timed out");

    let local_offer = offer.local_description().await.expect("local offer");
    let addrs = candidate_addresses(&local_offer.sdp);
    assert!(!addrs.is_empty(), "expected at least one candidate");
    for a in &addrs {
        assert!(
            a == "127.0.0.1" || a == "::1",
            "non-loopback candidate address: {a}"
        );
    }

    answer
        .set_remote_description(local_offer)
        .await
        .expect("set remote offer");
    let ans = answer.create_answer(None).await.expect("create answer");
    answer
        .set_local_description(ans)
        .await
        .expect("set local answer");
    timeout(Duration::from_secs(10), answer.gathering_complete())
        .await
        .expect("answer gathering timed out");
    let local_answer = answer.local_description().await.expect("local answer");
    for a in candidate_addresses(&local_answer.sdp) {
        assert!(
            a == "127.0.0.1" || a == "::1",
            "non-loopback answer candidate: {a}"
        );
    }
    offer
        .set_remote_description(local_answer)
        .await
        .expect("set remote answer");

    wait_connected(&offer).await;
    wait_connected(&answer).await;

    offer.close().await.expect("close");
    answer.close().await.expect("close");
}
