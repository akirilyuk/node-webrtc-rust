//! Peer connection configuration types.

use serde::{Deserialize, Serialize};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;

/// ICE server credential type (W3C `RTCIceCredentialType`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum IceCredentialType {
    #[default]
    Password,
    Oauth,
}

/// ICE server configuration (STUN/TURN).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IceServer {
    /// Server URLs, e.g. `stun:stun.l.google.com:19302`.
    pub urls: Vec<String>,
    /// Optional TURN username.
    pub username: Option<String>,
    /// Optional TURN credential.
    pub credential: Option<String>,
    /// Credential type (`password` or `oauth`). Only password is used by the underlying stack today.
    #[serde(default)]
    pub credential_type: IceCredentialType,
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
    /// When set, enables or disables `[webrtc-debug]` logging for this process.
    pub debug: Option<bool>,
}

impl PeerConnectionConfig {
    /// Applies optional debug override from configuration.
    pub fn apply_debug_override(&self) {
        if let Some(enabled) = self.debug {
            crate::debug::set_debug_enabled(enabled);
        }
    }

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

impl From<RTCConfiguration> for PeerConnectionConfig {
    fn from(config: RTCConfiguration) -> Self {
        Self {
            ice_servers: config
                .ice_servers
                .into_iter()
                .map(|server| IceServer {
                    urls: server.urls,
                    username: (!server.username.is_empty()).then_some(server.username),
                    credential: (!server.credential.is_empty()).then_some(server.credential),
                    credential_type: IceCredentialType::Password,
                })
                .collect(),
            ice_transport_policy: match config.ice_transport_policy {
                RTCIceTransportPolicy::Relay => IceTransportPolicy::Relay,
                _ => IceTransportPolicy::All,
            },
            debug: None,
        }
    }
}
