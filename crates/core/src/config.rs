//! Peer connection configuration types.

use serde::{Deserialize, Serialize};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;

/// ICE server configuration (STUN/TURN).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IceServer {
    /// Server URLs, e.g. `stun:stun.l.google.com:19302`.
    pub urls: Vec<String>,
    /// Optional TURN username.
    pub username: Option<String>,
    /// Optional TURN credential.
    pub credential: Option<String>,
}

/// Policy controlling which ICE candidates may be used.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum IceTransportPolicy {
    /// Use host, server-reflexive, and relay candidates.
    #[default]
    All,
    /// Use relay candidates only.
    Relay,
}

/// Configuration for creating a [`PeerConnection`](crate::PeerConnection).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerConnectionConfig {
    /// ICE servers available for connectivity checks.
    pub ice_servers: Vec<IceServer>,
    /// Which ICE candidate types are permitted.
    pub ice_transport_policy: IceTransportPolicy,
}

impl PeerConnectionConfig {
    /// Converts this configuration into a webrtc-rs `RTCConfiguration`.
    pub fn into_rtc_configuration(self) -> RTCConfiguration {
        RTCConfiguration {
            ice_servers: self
                .ice_servers
                .into_iter()
                .map(|server| RTCIceServer {
                    urls: server.urls,
                    username: server.username.unwrap_or_default(),
                    credential: server.credential.unwrap_or_default(),
                })
                .collect(),
            ice_transport_policy: match self.ice_transport_policy {
                IceTransportPolicy::All => RTCIceTransportPolicy::All,
                IceTransportPolicy::Relay => RTCIceTransportPolicy::Relay,
            },
            ..Default::default()
        }
    }
}
