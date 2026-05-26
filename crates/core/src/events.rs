//! Channel-based event subscriptions for peer connections.

use tokio::sync::mpsc;

use crate::data_channel::DataChannel;
use crate::media::RemoteTrack;
use crate::peer_connection::{ConnectionState, IceCandidate, IceConnectionState};

/// Receivers for all peer connection events.
pub struct PeerConnectionEvents {
    /// ICE candidates (`None` indicates gathering complete).
    pub ice_candidates: mpsc::UnboundedReceiver<Option<IceCandidate>>,
    /// Remote tracks as they are added.
    pub tracks: mpsc::UnboundedReceiver<RemoteTrack>,
    /// Incoming data channels.
    pub data_channels: mpsc::UnboundedReceiver<DataChannel>,
    /// Overall connection state changes.
    pub connection_state: mpsc::UnboundedReceiver<ConnectionState>,
    /// ICE connection state changes.
    pub ice_connection_state: mpsc::UnboundedReceiver<IceConnectionState>,
}

/// Senders for peer connection events (internal use).
pub(crate) struct PeerConnectionEventSenders {
    pub ice_candidates: mpsc::UnboundedSender<Option<IceCandidate>>,
    pub tracks: mpsc::UnboundedSender<RemoteTrack>,
    pub data_channels: mpsc::UnboundedSender<DataChannel>,
    pub connection_state: mpsc::UnboundedSender<ConnectionState>,
    pub ice_connection_state: mpsc::UnboundedSender<IceConnectionState>,
}

impl PeerConnectionEventSenders {
    pub fn new() -> (Self, PeerConnectionEvents) {
        let (ice_candidates_tx, ice_candidates_rx) = mpsc::unbounded_channel();
        let (tracks_tx, tracks_rx) = mpsc::unbounded_channel();
        let (data_channels_tx, data_channels_rx) = mpsc::unbounded_channel();
        let (connection_state_tx, connection_state_rx) = mpsc::unbounded_channel();
        let (ice_connection_state_tx, ice_connection_state_rx) = mpsc::unbounded_channel();

        (
            Self {
                ice_candidates: ice_candidates_tx,
                tracks: tracks_tx,
                data_channels: data_channels_tx,
                connection_state: connection_state_tx,
                ice_connection_state: ice_connection_state_tx,
            },
            PeerConnectionEvents {
                ice_candidates: ice_candidates_rx,
                tracks: tracks_rx,
                data_channels: data_channels_rx,
                connection_state: connection_state_rx,
                ice_connection_state: ice_connection_state_rx,
            },
        )
    }
}
