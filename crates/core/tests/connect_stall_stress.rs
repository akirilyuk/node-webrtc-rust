//! Investigation harness (ignored): loopback offerer/answerer pairs shaped like the helpers
//! SessionPod flow (host offers audio + two data channels, trickle both ways, client answers).
//! A pair that is not `connected` on both sides within the stall bound prints both sides' ICE,
//! DTLS and peer-connection states plus the candidate-pair stats.
//!
//! STRESS_ROUNDS=200 STRESS_PAIRS=3 cargo test -p node-webrtc-rust-core --test connect_stall_stress -- --ignored --nocapture

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::{APIBuilder, API};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::stats::StatsReportType;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn api() -> API {
    let mut m = MediaEngine::default();
    m.register_default_codecs().unwrap();
    let registry = register_default_interceptors(Registry::new(), &mut m).unwrap();
    let mut s = SettingEngine::default();
    if std::env::var("STRESS_LOOPBACK").as_deref() == Ok("1") {
        s.set_include_loopback_candidate(true);
    }
    APIBuilder::new()
        .with_setting_engine(s)
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build()
}

async fn describe(tag: &str, pc: &RTCPeerConnection) -> String {
    let mut out = format!(
        "{tag}: pc={} ice={} dtls={:?} sig={}\n",
        pc.connection_state(),
        pc.ice_connection_state(),
        pc.dtls_transport().state(),
        pc.signaling_state()
    );
    for (_, r) in pc.get_stats().await.reports {
        if let StatsReportType::LocalCandidate(c) = &r {
            out.push_str(&format!("  local {} {}:{}\n", c.id, c.ip, c.port));
        }
        if let StatsReportType::RemoteCandidate(c) = &r {
            out.push_str(&format!(
                "  remote {} {}:{} {:?}\n",
                c.id, c.ip, c.port, c.candidate_type
            ));
        }
        if let StatsReportType::CandidatePair(p) = r {
            out.push_str(&format!(
                "  pair {} state={:?} nominated={} req_tx={} req_rx={} resp_tx={} resp_rx={}\n",
                p.id,
                p.state,
                p.nominated,
                p.requests_sent,
                p.requests_received,
                p.responses_sent,
                p.responses_received
            ));
        }
    }
    out
}

fn wait_connected(pc: &Arc<RTCPeerConnection>) -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let tx = Arc::new(tx);
    pc.on_peer_connection_state_change(Box::new(move |s| {
        if s == RTCPeerConnectionState::Connected {
            let _ = tx.send(true);
        }
        Box::pin(async {})
    }));
    rx
}

async fn one_pair(api: Arc<API>, id: usize, stall: Duration) -> Result<Duration, String> {
    let cfg = RTCConfiguration::default();
    let host = Arc::new(api.new_peer_connection(cfg.clone()).await.unwrap());
    let client = Arc::new(api.new_peer_connection(cfg).await.unwrap());

    // Trickle: host -> client and client -> host over channels, applied in order.
    let (h2c_tx, mut h2c_rx) = mpsc::unbounded_channel::<RTCIceCandidateInit>();
    let (c2h_tx, mut c2h_rx) = mpsc::unbounded_channel::<RTCIceCandidateInit>();
    host.on_ice_candidate(Box::new(move |c| {
        if let Some(c) = c {
            let _ = h2c_tx.send(c.to_json().unwrap());
        }
        Box::pin(async {})
    }));
    client.on_ice_candidate(Box::new(move |c| {
        if let Some(c) = c {
            let _ = c2h_tx.send(c.to_json().unwrap());
        }
        Box::pin(async {})
    }));

    let mut host_conn = wait_connected(&host);
    let mut client_conn = wait_connected(&client);

    host.add_transceiver_from_kind(RTPCodecType::Audio, None)
        .await
        .unwrap();
    let _dc1 = host
        .create_data_channel("voice-control", None)
        .await
        .unwrap();
    let _dc2 = host
        .create_data_channel("voicethere-sync", None)
        .await
        .unwrap();

    let started = Instant::now();
    let offer = host.create_offer(None).await.unwrap();
    host.set_local_description(offer.clone()).await.unwrap();

    client.set_remote_description(offer).await.unwrap();
    let answer = client.create_answer(None).await.unwrap();
    client.set_local_description(answer.clone()).await.unwrap();

    let c = Arc::clone(&client);
    let pump_h2c = tokio::spawn(async move {
        while let Some(cand) = h2c_rx.recv().await {
            let _ = c.add_ice_candidate(cand).await;
        }
    });
    host.set_remote_description(answer).await.unwrap();
    let h = Arc::clone(&host);
    let pump_c2h = tokio::spawn(async move {
        while let Some(cand) = c2h_rx.recv().await {
            let _ = h.add_ice_candidate(cand).await;
        }
    });

    let mut both = Box::pin(async move {
        let _ = host_conn.wait_for(|v| *v).await;
        let _ = client_conn.wait_for(|v| *v).await;
    });
    let snapshot = Duration::from_millis(env_usize("STRESS_SNAPSHOT_MS", 2_000) as u64);
    let fast = tokio::time::timeout(snapshot, &mut both).await.is_ok();
    if !fast {
        eprintln!(
            "pair {id} SLOW at {:?}\n{}{}",
            snapshot,
            describe("host(offerer)", &host).await,
            describe("client(answerer)", &client).await
        );
    }
    let rest = async {
        if !fast {
            both.await
        }
    };
    let result = match tokio::time::timeout(stall.saturating_sub(snapshot), rest).await {
        Ok(()) => Ok(started.elapsed()),
        Err(_) => Err(format!(
            "pair {id} STALL after {:?}\n{}{}",
            stall,
            describe("host(offerer)", &host).await,
            describe("client(answerer)", &client).await
        )),
    };
    pump_h2c.abort();
    pump_c2h.abort();
    let _ = host.close().await;
    let _ = client.close().await;
    result
}

#[test]
#[ignore = "investigation stress harness"]
fn connect_stall_stress() {
    let _ = env_logger::try_init();
    let rounds = env_usize("STRESS_ROUNDS", 50);
    let pairs = env_usize("STRESS_PAIRS", 3);
    let workers = env_usize("STRESS_WORKERS", 4);
    let stall = Duration::from_millis(env_usize("STRESS_STALL_MS", 8_000) as u64);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let api = Arc::new(api());
        let mut stalls = 0usize;
        let mut times = vec![];
        for round in 0..rounds {
            let mut set = tokio::task::JoinSet::new();
            for p in 0..pairs {
                set.spawn(one_pair(Arc::clone(&api), round * pairs + p, stall));
            }
            while let Some(r) = set.join_next().await {
                match r.unwrap() {
                    Ok(t) => times.push(t),
                    Err(msg) => {
                        stalls += 1;
                        eprintln!("{msg}");
                    }
                }
            }
        }
        times.sort();
        let p = |q: f64| times.get(((times.len() as f64) * q) as usize).copied();
        eprintln!(
            "connect_stall_stress: {} ok, {} stalls, p50={:?} p99={:?} max={:?}",
            times.len(),
            stalls,
            p(0.5),
            p(0.99),
            times.last()
        );
        assert_eq!(stalls, 0, "connect stalls observed");
    });
}
