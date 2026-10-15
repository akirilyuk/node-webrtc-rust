//! Guard for net.ice-nomination-never-answered (node-webrtc-rust vendored patch): the controlling
//! agent used to re-nominate the same pair every 200 ms forever when the controlled agent's
//! socket for that pair stopped completing writes right after it selected the pair, and a data
//! write (DTLS ClientHello) on such a socket hung without a bound. The peer stayed in `checking`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use stun::attributes::ATTR_USE_CANDIDATE;
use stun::message::*;
use tokio::net::UdpSocket;
use util::Conn;

use super::agent_stun_send_test::{host_candidate, NeverWritableConn};
use super::*;
use crate::candidate::candidate_base::*;
use crate::candidate::candidate_host::*;
use crate::use_candidate::UseCandidateAttr;

const PRIORITY_A: u32 = 2_000_000_000;
const PRIORITY_B: u32 = 1_000_000_000;
const PRIORITY_C: u32 = 1_500_000_000;

/// A real UDP socket with two test hooks on the receive path:
/// - `wedge_on_use_candidate`: the moment a STUN request carrying USE-CANDIDATE arrives, every
///   later write hangs forever (receiving keeps working), like the macOS socket that stopped
///   draining right after the controlled agent selected the nominated pair.
/// - `drop_requests_until`: inbound STUN requests are dropped until that flag is set, so the
///   pair is not valid on the controlling side before the first pair got nominated.
struct WedgeableConn {
    sock: UdpSocket,
    wedged: Arc<AtomicBool>,
    wedge_on_use_candidate: bool,
    drop_requests_until: Option<Arc<AtomicBool>>,
}

impl WedgeableConn {
    async fn wait_if_wedged(&self) {
        if self.wedged.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl Conn for WedgeableConn {
    async fn connect(&self, _addr: SocketAddr) -> util::Result<()> {
        Ok(())
    }
    async fn recv(&self, buf: &mut [u8]) -> util::Result<usize> {
        Ok(self.recv_from(buf).await?.0)
    }
    async fn recv_from(&self, buf: &mut [u8]) -> util::Result<(usize, SocketAddr)> {
        loop {
            let (n, from) = self.sock.recv_from(buf).await?;
            if is_message(&buf[..n]) {
                let mut m = Message {
                    raw: buf[..n].to_vec(),
                    ..Message::default()
                };
                if m.decode().is_ok() {
                    if m.typ.class == CLASS_REQUEST {
                        if let Some(gate) = &self.drop_requests_until {
                            if !gate.load(Ordering::SeqCst) {
                                continue;
                            }
                        }
                    }
                    if self.wedge_on_use_candidate && m.contains(ATTR_USE_CANDIDATE) {
                        self.wedged.store(true, Ordering::SeqCst);
                    }
                }
            }
            return Ok((n, from));
        }
    }
    async fn send(&self, buf: &[u8]) -> util::Result<usize> {
        self.wait_if_wedged().await;
        Ok(self.sock.send(buf).await?)
    }
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> util::Result<usize> {
        self.wait_if_wedged().await;
        Ok(self.sock.send_to(buf, target).await?)
    }
    fn local_addr(&self) -> util::Result<SocketAddr> {
        Ok(self.sock.local_addr()?)
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
    async fn close(&self) -> util::Result<()> {
        Ok(())
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

fn host_candidate_with_priority(
    addr: SocketAddr,
    conn: Option<Arc<dyn Conn + Send + Sync>>,
    priority: u32,
) -> Arc<dyn Candidate + Send + Sync> {
    Arc::new(
        CandidateHostConfig {
            base_config: CandidateBaseConfig {
                network: "udp".to_owned(),
                address: addr.ip().to_string(),
                port: addr.port(),
                component: 1,
                priority,
                conn,
                ..Default::default()
            },
            ..Default::default()
        }
        .new_candidate_host()
        .unwrap(),
    )
}

async fn eventually(deadline: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cond()
}

#[tokio::test]
async fn nominated_pair_whose_socket_wedges_fails_over_to_the_next_pair() -> Result<()> {
    let started = Instant::now();

    // Controlling agent C: one candidate on a plain socket.
    let sock_c = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let addr_c = sock_c.local_addr()?;
    let conn_c: Arc<dyn Conn + Send + Sync> = sock_c;
    let cand_c = host_candidate_with_priority(addr_c, Some(conn_c), PRIORITY_C);

    // Controlled agent D: candidate A (higher priority) wedges the moment it receives the
    // nomination; candidate B answers only once A is wedged, so A is the only valid pair
    // (and so the only one C can nominate first) and B is valid by the time C fails over.
    let wedged_a = Arc::new(AtomicBool::new(false));
    let conn_a = WedgeableConn {
        sock: UdpSocket::bind("127.0.0.1:0").await?,
        wedged: Arc::clone(&wedged_a),
        wedge_on_use_candidate: true,
        drop_requests_until: None,
    };
    let conn_b = WedgeableConn {
        sock: UdpSocket::bind("127.0.0.1:0").await?,
        wedged: Arc::new(AtomicBool::new(false)),
        wedge_on_use_candidate: false,
        drop_requests_until: Some(Arc::clone(&wedged_a)),
    };
    let addr_a = conn_a.local_addr()?;
    let addr_b = conn_b.local_addr()?;
    let cand_a = host_candidate_with_priority(addr_a, Some(Arc::new(conn_a)), PRIORITY_A);
    let cand_b = host_candidate_with_priority(addr_b, Some(Arc::new(conn_b)), PRIORITY_B);

    let c = Agent::new(AgentConfig::default()).await?;
    let d = Agent::new(AgentConfig::default()).await?;

    // Local candidates added by hand, remote candidates are conn-less views of the peer's.
    c.internal.add_candidate(&cand_c).await?;
    d.internal.add_candidate(&cand_a).await?;
    d.internal.add_candidate(&cand_b).await?;
    c.internal
        .add_remote_candidate(&host_candidate_with_priority(addr_a, None, PRIORITY_A))
        .await;
    c.internal
        .add_remote_candidate(&host_candidate_with_priority(addr_b, None, PRIORITY_B))
        .await;
    d.internal
        .add_remote_candidate(&host_candidate_with_priority(addr_c, None, PRIORITY_C))
        .await;

    let (c_ufrag, c_pwd) = c.get_local_user_credentials().await;
    let (d_ufrag, d_pwd) = d.get_local_user_credentials().await;
    let (_c_cancel_tx, c_cancel_rx) = mpsc::channel(1);
    let (_d_cancel_tx, d_cancel_rx) = mpsc::channel(1);

    let dial = c.dial(c_cancel_rx, d_ufrag, d_pwd);
    let accept = d.accept(d_cancel_rx, c_ufrag, c_pwd);
    let (c_conn, d_conn) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(dial, accept)
    })
    .await
    .expect("both agents must connect within 10 s")?;

    assert!(
        wedged_a.load(Ordering::SeqCst),
        "pair A was never nominated first, the test did not exercise the failover"
    );

    let c_pair = c
        .get_selected_candidate_pair()
        .expect("C has a selected pair");
    assert_eq!(c_pair.remote.addr(), addr_b, "C stayed on the wedged pair");

    // The controlled side learns about the new nomination when B's USE-CANDIDATE arrives and
    // is answered; that can land just after C's dial returned.
    assert!(
        eventually(
            Duration::from_secs(10).saturating_sub(started.elapsed()),
            || {
                d.get_selected_candidate_pair()
                    .is_some_and(|p| p.local.addr() == addr_b)
            }
        )
        .await,
        "D stayed on the wedged pair, selected: {:?}",
        d.get_selected_candidate_pair().map(|p| p.local.addr())
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "failover took {:?}",
        started.elapsed()
    );

    // Data written by D after the switch arrives at C.
    let payload: Vec<u8> = (0..40u8).map(|i| if i == 0 { 0x16 } else { i }).collect();
    d_conn.send(&payload).await?;
    let mut buf = [0u8; 1500];
    let n = tokio::time::timeout(Duration::from_secs(2), c_conn.recv(&mut buf))
        .await
        .expect("data from D never reached C")?;
    assert_eq!(&buf[..n], payload.as_slice());

    c.close().await?;
    d.close().await?;
    Ok(())
}

#[tokio::test]
async fn data_write_on_a_wedged_selected_pair_returns_within_the_bound() -> Result<()> {
    let a = Agent::new(AgentConfig::default()).await?;
    let stuck = host_candidate(
        "127.0.0.1:9".parse().unwrap(),
        Some(Arc::new(NeverWritableConn {
            local: "127.0.0.1:9".parse().unwrap(),
        })),
    );
    let remote = host_candidate("127.0.0.1:10".parse().unwrap(), None);
    a.internal
        .agent_conn
        .selected_pair
        .store(Some(Arc::new(CandidatePair::new(stuck, remote, false))));

    // A DTLS-looking record (content type 22), not a STUN message.
    let mut record = vec![0x16, 0xfe, 0xfd];
    record.resize(60, 0xab);

    let started = Instant::now();
    let sent = tokio::time::timeout(Duration::from_secs(2), a.internal.agent_conn.send(&record))
        .await
        .expect("AgentConn::send never returned");
    assert_eq!(sent?, record.len());
    assert!(
        started.elapsed() < DATA_SEND_TIMEOUT * 5,
        "send took {:?}",
        started.elapsed()
    );
    assert_eq!(
        a.internal.agent_conn.send_timeouts.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        a.internal.agent_conn.bytes_sent(),
        0,
        "a dropped write must not count as sent"
    );

    a.close().await?;
    Ok(())
}

#[tokio::test]
async fn controlled_switches_to_a_new_nomination() -> Result<()> {
    let a = Agent::new(AgentConfig::default()).await?;
    assert!(!a.internal.is_controlling.load(Ordering::SeqCst));

    let mut locals = vec![];
    let mut remotes = vec![];
    for _ in 0..2 {
        let sock: Arc<dyn Conn + Send + Sync> = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        locals.push(host_candidate(sock.local_addr()?, Some(sock)));
        let peer = UdpSocket::bind("127.0.0.1:0").await?;
        remotes.push(host_candidate(peer.local_addr()?, None));
    }
    for (l, r) in locals.iter().zip(&remotes) {
        a.internal.add_pair(l.clone(), r.clone()).await;
    }
    let p1 = a.internal.find_pair(&locals[0], &remotes[0]).await.unwrap();
    let p2 = a.internal.find_pair(&locals[1], &remotes[1]).await.unwrap();
    p1.state
        .store(CandidatePairState::Succeeded as u8, Ordering::SeqCst);
    p2.state
        .store(CandidatePairState::Succeeded as u8, Ordering::SeqCst);

    a.internal.set_selected_pair(Some(Arc::clone(&p1))).await;
    assert!(Arc::ptr_eq(
        &a.internal.agent_conn.get_selected_pair().unwrap(),
        &p1
    ));

    // The controlling agent nominates the second pair.
    let mut m = Message::new();
    m.build(&[
        Box::new(BINDING_REQUEST),
        Box::new(TransactionId::new()),
        Box::<UseCandidateAttr>::default(),
    ])?;
    a.internal
        .handle_binding_request(&m, &locals[1], &remotes[1])
        .await;
    assert!(
        Arc::ptr_eq(&a.internal.agent_conn.get_selected_pair().unwrap(), &p2),
        "controlled agent kept its first selected pair"
    );

    // The same nomination again changes nothing.
    a.internal
        .handle_binding_request(&m, &locals[1], &remotes[1])
        .await;
    assert!(Arc::ptr_eq(
        &a.internal.agent_conn.get_selected_pair().unwrap(),
        &p2
    ));

    a.close().await?;
    Ok(())
}
