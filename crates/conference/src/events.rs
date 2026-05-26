//! Channel-based event subscriptions for conference rooms.

use tokio::sync::mpsc;

use crate::mute::MuteScope;

/// Participant joined the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantJoined {
    pub participant_id: String,
}

/// Participant left the room (not kicked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantLeft {
    pub participant_id: String,
}

/// Participant was kicked from the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantKicked {
    pub participant_id: String,
    pub reason: Option<String>,
}

/// Participant mute state changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantMuted {
    pub target_id: String,
    pub scope: MuteScope,
    pub listener_id: Option<String>,
}

/// Room-wide mixing enabled flag changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixingEnabledChanged {
    pub enabled: bool,
}

/// Room-level error surfaced to bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomError {
    pub message: String,
    pub code: Option<String>,
}

/// Receivers for all room events.
pub struct RoomEvents {
    pub participant_joined: mpsc::UnboundedReceiver<ParticipantJoined>,
    pub participant_left: mpsc::UnboundedReceiver<ParticipantLeft>,
    pub participant_kicked: mpsc::UnboundedReceiver<ParticipantKicked>,
    pub participant_muted: mpsc::UnboundedReceiver<ParticipantMuted>,
    pub mixing_enabled_changed: mpsc::UnboundedReceiver<MixingEnabledChanged>,
    pub error: mpsc::UnboundedReceiver<RoomError>,
}

/// Senders for room events (internal use).
pub(crate) struct RoomEventSenders {
    pub participant_joined: mpsc::UnboundedSender<ParticipantJoined>,
    pub participant_left: mpsc::UnboundedSender<ParticipantLeft>,
    pub participant_kicked: mpsc::UnboundedSender<ParticipantKicked>,
    pub participant_muted: mpsc::UnboundedSender<ParticipantMuted>,
    pub mixing_enabled_changed: mpsc::UnboundedSender<MixingEnabledChanged>,
    pub error: mpsc::UnboundedSender<RoomError>,
}

impl RoomEventSenders {
    pub fn new() -> (Self, RoomEvents) {
        let (joined_tx, joined_rx) = mpsc::unbounded_channel();
        let (left_tx, left_rx) = mpsc::unbounded_channel();
        let (kicked_tx, kicked_rx) = mpsc::unbounded_channel();
        let (muted_tx, muted_rx) = mpsc::unbounded_channel();
        let (mixing_tx, mixing_rx) = mpsc::unbounded_channel();
        let (error_tx, error_rx) = mpsc::unbounded_channel();

        (
            Self {
                participant_joined: joined_tx,
                participant_left: left_tx,
                participant_kicked: kicked_tx,
                participant_muted: muted_tx,
                mixing_enabled_changed: mixing_tx,
                error: error_tx,
            },
            RoomEvents {
                participant_joined: joined_rx,
                participant_left: left_rx,
                participant_kicked: kicked_rx,
                participant_muted: muted_rx,
                mixing_enabled_changed: mixing_rx,
                error: error_rx,
            },
        )
    }
}
