//! Guard for net.stun-request-no-retransmit (node-webrtc-rust vendored patch): `stun_request`
//! used to send one Binding request and wait the whole gather bound (5 s) on a single read, so
//! one lost UDP datagram cost 5 s and the srflx candidate, and the first datagram on the socket
//! was taken as the answer whatever its transaction id. RFC 5389 section 7.2.1: retransmit with
//! RTO doubling (sends at 0, 0.5, 1.5, 3.5 s), same transaction id, stop at the caller's deadline.
//! Every test uses a real UDP STUN responder on 127.0.0.1.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};

use super::*;
use crate::agent::agent_config::AgentConfig;
use crate::agent::Agent;
use crate::candidate::*;
use crate::url::Url;

const MAPPED_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
const MAPPED_PORT: u16 = 4242;
const GATHER_DEADLINE: Duration = Duration::from_secs(5);

fn decode_request(raw: &[u8]) -> Message {
    let mut m = Message::new();
    m.raw = raw.to_vec();
    m.decode().expect("responder received a STUN message");
    m
}

fn binding_success(tid: TransactionId, ip: IpAddr, port: u16) -> Vec<u8> {
    let mut m = Message::new();
    m.build(&[
        Box::new(BINDING_SUCCESS),
        Box::new(tid),
        Box::new(XorMappedAddress { ip, port }),
    ])
    .unwrap();
    m.raw
}

/// What the responder does with the n-th (0-based) Binding request it receives.
#[derive(Clone, Copy)]
enum Reply {
    Drop,
    Answer,
    /// Answer after a delay (a late answer to this transmission).
    AnswerAfter(Duration),
    /// Send a non-STUN datagram and a Binding success with another transaction id first, then
    /// the real answer.
    StrayThenAnswer,
}

struct Responder {
    addr: SocketAddr,
    received: Arc<AtomicUsize>,
}

async fn spawn_responder(policy: fn(usize) -> Reply) -> Responder {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    let received = Arc::new(AtomicUsize::new(0));
    let received2 = Arc::clone(&received);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                return;
            };
            let req = decode_request(&buf[..n]);
            let idx = received2.fetch_add(1, Ordering::SeqCst);
            let answer = binding_success(req.transaction_id, MAPPED_IP, MAPPED_PORT);
            match policy(idx) {
                Reply::Drop => {}
                Reply::Answer => {
                    let _ = sock.send_to(&answer, from).await;
                }
                Reply::AnswerAfter(delay) => {
                    let sock2 = Arc::clone(&sock);
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = sock2.send_to(&answer, from).await;
                    });
                }
                Reply::StrayThenAnswer => {
                    let _ = sock.send_to(b"not a stun message", from).await;
                    let stray = binding_success(
                        TransactionId::new(),
                        IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
                        9,
                    );
                    let _ = sock.send_to(&stray, from).await;
                    let _ = sock.send_to(&answer, from).await;
                }
            }
        }
    });
    Responder { addr, received }
}

async fn client_conn() -> Arc<dyn Conn + Send + Sync> {
    Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap())
}

#[tokio::test]
async fn test_stun_request_retransmits_after_lost_first_request() -> Result<()> {
    let responder = spawn_responder(|i| if i == 0 { Reply::Drop } else { Reply::Answer }).await;
    let conn = client_conn().await;

    let start = Instant::now();
    let addr = get_xormapped_addr(&conn, responder.addr, GATHER_DEADLINE).await?;
    let elapsed = start.elapsed();

    assert_eq!(addr.ip, MAPPED_IP);
    assert_eq!(addr.port, MAPPED_PORT);
    assert!(
        elapsed < Duration::from_secs(2),
        "a lost first request must be retransmitted, took {elapsed:?}"
    );
    assert!(responder.received.load(Ordering::SeqCst) >= 2);
    Ok(())
}

#[tokio::test]
async fn test_gather_srflx_survives_lost_first_request() -> Result<()> {
    let responder = spawn_responder(|i| if i == 0 { Reply::Drop } else { Reply::Answer }).await;

    let a = Agent::new(AgentConfig {
        urls: vec![Url::parse_url(&format!(
            "stun:127.0.0.1:{}",
            responder.addr.port()
        ))?],
        network_types: vec![NetworkType::Udp4],
        candidate_types: vec![CandidateType::ServerReflexive],
        ..Default::default()
    })
    .await?;

    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);
    let done_tx = Arc::new(Mutex::new(Some(done_tx)));
    a.on_candidate(Box::new(
        move |c: Option<Arc<dyn Candidate + Send + Sync>>| {
            let done_tx = Arc::clone(&done_tx);
            Box::pin(async move {
                if c.is_none() {
                    done_tx.lock().await.take();
                }
            })
        },
    ));

    let start = Instant::now();
    a.gather_candidates()?;
    tokio::time::timeout(Duration::from_secs(10), done_rx.recv())
        .await
        .expect("gathering completes");
    let elapsed = start.elapsed();

    let srflx: Vec<_> = a
        .get_local_candidates()
        .await?
        .into_iter()
        .filter(|c| c.candidate_type() == CandidateType::ServerReflexive)
        .collect();
    a.close().await?;

    assert_eq!(srflx.len(), 1, "one srflx candidate");
    assert_eq!(srflx[0].address(), MAPPED_IP.to_string());
    assert_eq!(srflx[0].port(), MAPPED_PORT);
    assert!(
        elapsed < Duration::from_secs(2),
        "gathering must not wait out the 5 s bound after one lost request, took {elapsed:?}"
    );
    Ok(())
}

#[tokio::test]
async fn test_stun_request_gives_up_at_deadline_after_four_sends() -> Result<()> {
    // The test owns the server socket: every datagram the client sent before the call returned
    // is in its receive buffer, so the count needs no wait.
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let conn = client_conn().await;

    let start = Instant::now();
    let res = stun_request(&conn, server.local_addr().unwrap(), GATHER_DEADLINE).await;
    let elapsed = start.elapsed();

    assert!(res.is_err(), "no answer must be an error");
    assert!(
        elapsed <= GATHER_DEADLINE + Duration::from_millis(300),
        "the caller's 5 s bound is kept, took {elapsed:?}"
    );
    assert!(
        elapsed >= GATHER_DEADLINE - Duration::from_millis(50),
        "waits for the whole bound, took {elapsed:?}"
    );

    let mut buf = vec![0u8; 1500];
    let mut tids = Vec::new();
    while let Ok((n, _)) = server.try_recv_from(&mut buf) {
        tids.push(decode_request(&buf[..n]).transaction_id);
    }
    assert_eq!(tids.len(), 4, "sends at 0, 0.5, 1.5 and 3.5 s");
    assert!(
        tids.iter().all(|t| *t == tids[0]),
        "every retransmission carries the same transaction id"
    );
    Ok(())
}

#[tokio::test]
async fn test_stun_request_accepts_late_answer_to_first_transmission() -> Result<()> {
    // Only the first transmission is answered, after the second send went out.
    let responder = spawn_responder(|i| {
        if i == 0 {
            Reply::AnswerAfter(Duration::from_millis(700))
        } else {
            Reply::Drop
        }
    })
    .await;
    let conn = client_conn().await;

    let start = Instant::now();
    let addr = get_xormapped_addr(&conn, responder.addr, GATHER_DEADLINE).await?;
    let elapsed = start.elapsed();

    assert_eq!(addr.ip, MAPPED_IP);
    assert!(
        elapsed < Duration::from_secs(2),
        "the late answer is accepted, took {elapsed:?}"
    );
    Ok(())
}

#[tokio::test]
async fn test_stun_request_ignores_stray_datagrams() -> Result<()> {
    let responder = spawn_responder(|_| Reply::StrayThenAnswer).await;
    let conn = client_conn().await;

    let addr = get_xormapped_addr(&conn, responder.addr, GATHER_DEADLINE).await?;

    assert_eq!(
        addr.ip, MAPPED_IP,
        "the answer with the request's transaction id is used, not the stray one"
    );
    assert_eq!(addr.port, MAPPED_PORT);
    Ok(())
}
