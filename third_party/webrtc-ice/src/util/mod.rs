#[cfg(test)]
mod stun_retransmit_test;
#[cfg(test)]
mod util_test;

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use stun::agent::*;
use stun::attributes::*;
use stun::integrity::*;
use stun::message::*;
use stun::textattrs::*;
use stun::xoraddr::*;
use tokio::time::Duration;
use util::vnet::net::*;
use util::Conn;

use crate::agent::agent_config::{InterfaceFilterFn, IpFilterFn};
use crate::error::*;
use crate::network_type::*;

pub fn create_addr(_network: NetworkType, ip: IpAddr, port: u16) -> SocketAddr {
    /*if network.is_tcp(){
        return &net.TCPAddr{IP: ip, Port: port}
    default:
        return &net.UDPAddr{IP: ip, Port: port}
    }*/
    SocketAddr::new(ip, port)
}

pub fn assert_inbound_username(m: &Message, expected_username: &str) -> Result<()> {
    let mut username = Username::new(ATTR_USERNAME, String::new());
    username.get_from(m)?;

    if username.to_string() != expected_username {
        return Err(Error::Other(format!(
            "{:?} expected({}) actual({})",
            Error::ErrMismatchUsername,
            expected_username,
            username,
        )));
    }

    Ok(())
}

pub fn assert_inbound_message_integrity(m: &mut Message, key: &[u8]) -> Result<()> {
    let message_integrity_attr = MessageIntegrity(key.to_vec());
    Ok(message_integrity_attr.check(m)?)
}

/// Initiates a stun requests to `server_addr` using conn, reads the response and returns the
/// `XORMappedAddress` returned by the stun server.
/// Adapted from stun v0.2.
pub async fn get_xormapped_addr(
    conn: &Arc<dyn Conn + Send + Sync>,
    server_addr: SocketAddr,
    deadline: Duration,
) -> Result<XorMappedAddress> {
    let resp = stun_request(conn, server_addr, deadline).await?;
    let mut addr = XorMappedAddress::default();
    addr.get_from(&resp)?;
    Ok(addr)
}

const MAX_MESSAGE_SIZE: usize = 1280;

/// Initial retransmission timeout for a Binding request (RFC 5389 section 7.2.1, RTO = 500 ms).
/// Each retransmission doubles it: sends at 0, 0.5, 1.5, 3.5 s, ... until the caller's deadline.
pub(crate) const STUN_REQUEST_INITIAL_RTO: Duration = Duration::from_millis(500);

/// Sends a Binding request to `server_addr` and returns the matching response.
///
/// node-webrtc-rust vendored patch: upstream sent one request and took the first datagram that
/// arrived within `deadline`, so one lost datagram cost the whole deadline (5 s during gathering)
/// and any stray datagram was decoded as the answer. Now:
/// - `deadline > 0`: the request is retransmitted with the same transaction id, starting at
///   `STUN_REQUEST_INITIAL_RTO` and doubling (RFC 5389 section 7.2.1), and the call never takes
///   longer than `deadline` in total.
/// - `deadline == 0`: one send and an unbounded wait (no caller uses this today).
///
/// In both modes only a datagram that decodes as STUN, is a Binding success or error response
/// and carries the request's transaction id is accepted; anything else on `conn` is ignored and
/// the wait continues with the time that is left.
pub async fn stun_request(
    conn: &Arc<dyn Conn + Send + Sync>,
    server_addr: SocketAddr,
    deadline: Duration,
) -> Result<Message> {
    let mut request = Message::new();
    request.build(&[Box::new(BINDING_REQUEST), Box::new(TransactionId::new())])?;
    let tid = request.transaction_id;

    let mut bs = vec![0_u8; MAX_MESSAGE_SIZE];

    if deadline == Duration::from_secs(0) {
        conn.send_to(&request.raw, server_addr).await?;
        loop {
            let (n, _) = conn.recv_from(&mut bs).await?;
            if let Some(res) = decode_binding_response(&bs[..n], &tid) {
                return Ok(res);
            }
        }
    }

    let start = tokio::time::Instant::now();
    let end = start + deadline;
    let mut rto = STUN_REQUEST_INITIAL_RTO;
    let mut next_send = start + rto;
    let mut sends = 1_u32;

    conn.send_to(&request.raw, server_addr).await?;
    loop {
        let wake = next_send.min(end);
        match tokio::time::timeout_at(wake, conn.recv_from(&mut bs)).await {
            Ok(Ok((n, _))) => {
                if let Some(res) = decode_binding_response(&bs[..n], &tid) {
                    return Ok(res);
                }
                // Not the answer to this request: keep waiting with the time that is left.
            }
            Ok(Err(err)) => return Err(Error::Other(err.to_string())),
            Err(_) => {
                if tokio::time::Instant::now() >= end {
                    return Err(Error::Other(format!(
                        "stun request to {server_addr}: no response after {sends} transmissions in {deadline:?}"
                    )));
                }
                // Retransmit. A send that fails or does not finish before the deadline counts as
                // a lost datagram; the next timer decides.
                match tokio::time::timeout_at(end, conn.send_to(&request.raw, server_addr)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(err)) => {
                        log::debug!("stun request to {server_addr}: retransmission failed: {err}")
                    }
                    Err(_) => {}
                }
                sends += 1;
                rto *= 2;
                next_send += rto;
            }
        }
    }
}

/// Returns the decoded message if `raw` is a Binding success or error response for `tid`.
fn decode_binding_response(raw: &[u8], tid: &TransactionId) -> Option<Message> {
    let mut res = Message::new();
    res.raw = raw.to_vec();
    if res.decode().is_err() {
        return None;
    }
    if res.transaction_id != *tid
        || res.typ.method != METHOD_BINDING
        || (res.typ.class != CLASS_SUCCESS_RESPONSE && res.typ.class != CLASS_ERROR_RESPONSE)
    {
        return None;
    }
    Some(res)
}

pub async fn local_interfaces(
    vnet: &Arc<Net>,
    interface_filter: &Option<InterfaceFilterFn>,
    ip_filter: &Option<IpFilterFn>,
    network_types: &[NetworkType],
    include_loopback: bool,
) -> HashSet<IpAddr> {
    let mut ips = HashSet::new();
    let interfaces = vnet.get_interfaces().await;

    let (mut ipv4requested, mut ipv6requested) = (false, false);
    for typ in network_types {
        if typ.is_ipv4() {
            ipv4requested = true;
        }
        if typ.is_ipv6() {
            ipv6requested = true;
        }
    }

    for iface in interfaces {
        if let Some(filter) = interface_filter {
            if !filter(iface.name()) {
                continue;
            }
        }

        for ipnet in iface.addrs() {
            let ipaddr = ipnet.addr();

            if (!ipaddr.is_loopback() || include_loopback)
                && ((ipv4requested && ipaddr.is_ipv4()) || (ipv6requested && ipaddr.is_ipv6()))
                && ip_filter
                    .as_ref()
                    .map(|filter| filter(ipaddr))
                    .unwrap_or(true)
            {
                ips.insert(ipaddr);
            }
        }
    }

    ips
}

pub async fn listen_udp_in_port_range(
    vnet: &Arc<Net>,
    port_max: u16,
    port_min: u16,
    laddr: SocketAddr,
) -> Result<Arc<dyn Conn + Send + Sync>> {
    if laddr.port() != 0 || (port_min == 0 && port_max == 0) {
        return Ok(vnet.bind(laddr).await?);
    }
    let i = if port_min == 0 { 1 } else { port_min };
    let j = if port_max == 0 { 0xFFFF } else { port_max };
    if i > j {
        return Err(Error::ErrPort);
    }

    let port_start = rand::random::<u16>() % (j - i + 1) + i;
    let mut port_current = port_start;
    loop {
        let laddr = SocketAddr::new(laddr.ip(), port_current);
        match vnet.bind(laddr).await {
            Ok(c) => return Ok(c),
            Err(err) => log::debug!("failed to listen {laddr}: {err}"),
        };

        port_current = port_current.checked_add(1).unwrap_or(i);

        if port_current > j {
            port_current = i;
        }
        if port_current == port_start {
            break;
        }
    }

    Err(Error::ErrPort)
}

#[cfg(test)]
mod tests {
    use util::vnet::net::{Net, NetConfig};

    use crate::util::listen_udp_in_port_range;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_listen_udp_in_port_range_overflow() {
        let vnet = Arc::new(Net::new(Some(NetConfig {
            static_ips: vec![],
            static_ip: "127.0.0.1".parse().unwrap(),
        })));

        // Preemptively bind to port 65535 (u16::MAX) to ensure that listen_udp_in_port_range
        // triggers overflow logic by being unable to bind to port 65535 itself.
        let _conn = vnet
            .bind("127.0.0.1:65535".parse().unwrap())
            .await
            .expect("Expected binding to vnet to succeed");

        let port_max = u16::MAX;
        let port_min = u16::MAX;

        assert!(matches!(
            listen_udp_in_port_range(&vnet, port_max, port_min, "127.0.0.1:0".parse().unwrap())
                .await,
            Err(crate::Error::ErrPort)
        ))
    }
}
