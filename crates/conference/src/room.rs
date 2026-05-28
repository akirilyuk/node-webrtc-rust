//! Conference room lifecycle, participant map, and mixing controls.

use std::collections::HashMap;
use std::sync::Arc;

use node_webrtc_rust_core::{debug_call, debug_evt, IceServer, PeerConnection, PeerConnectionConfig};
use node_webrtc_rust_mixer::{Frame, MixGraph};
use tokio::sync::Mutex;

use crate::error::ConferenceError;
use crate::events::{MixingEnabledChanged, ParticipantJoined, ParticipantKicked, ParticipantLeft, ParticipantMuted, RoomEventSenders, RoomEvents};
use crate::mute::{MuteMatrix, MuteScope};
use crate::participant::Participant;
use crate::signaling::{SignalingMessage, SignalingResponse};

/// Room configuration options.
#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub max_participants: usize,
    pub ice_servers: Vec<IceServer>,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            max_participants: 32,
            ice_servers: Vec::new(),
        }
    }
}

/// Participant summary for list APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantInfo {
    pub id: String,
    pub connection_state: String,
}

/// One conference room with shared mixing graph and participants.
pub struct Room {
    id: String,
    participants: HashMap<String, Participant>,
    mix_graph: Arc<Mutex<MixGraph>>,
    mute_matrix: MuteMatrix,
    config: RoomConfig,
    event_senders: Option<RoomEventSenders>,
}

impl Room {
    /// Creates an empty room with a fresh mix graph.
    pub fn new(id: impl Into<String>, config: RoomConfig) -> Self {
        let id = id.into();
        debug_call!("conference::room", "new", "id={id}");
        let mix_graph = Arc::new(Mutex::new(MixGraph::new()));
        let mute_matrix = MuteMatrix::new(Arc::clone(&mix_graph));
        Self {
            id,
            participants: HashMap::new(),
            mix_graph,
            mute_matrix,
            config,
            event_senders: None,
        }
    }

    /// Subscribes to room lifecycle and control events via async channels.
    pub fn subscribe_events(&mut self) -> RoomEvents {
        let (senders, events) = RoomEventSenders::new();
        self.event_senders = Some(senders);
        events
    }

    /// Returns the room identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns room configuration.
    pub fn config(&self) -> &RoomConfig {
        &self.config
    }

    /// Returns whether room-wide mixing is enabled.
    pub async fn mixing_enabled(&self) -> bool {
        self.mute_matrix.mixing_enabled().await
    }

    /// Enables or disables room-wide mixing.
    pub async fn set_mixing_enabled(&self, enabled: bool) {
        debug_call!(
            "conference::room",
            "set_mixing_enabled",
            "id={}, enabled={enabled}",
            self.id
        );
        self.mute_matrix.set_mixing_enabled(enabled).await;
        debug_evt!(
            "conference::room",
            "mixing_enabled_changed",
            "id={}, enabled={enabled}",
            self.id
        );
        self.emit_mixing_enabled_changed(enabled);
    }

    /// Adds a participant with a fresh peer connection and mixer slot.
    pub async fn add_participant(&mut self, participant_id: &str) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::room",
            "add_participant",
            "room={}, participant={participant_id}",
            self.id
        );

        if self.participants.contains_key(participant_id) {
            return Err(ConferenceError::internal(format!(
                "participant {participant_id} already in room"
            )));
        }

        if self.participants.len() >= self.config.max_participants {
            return Err(ConferenceError::room_full(format!(
                "room {} is full (max {})",
                self.id, self.config.max_participants
            )));
        }

        let pc_config = PeerConnectionConfig {
            ice_servers: self.config.ice_servers.clone(),
            ..Default::default()
        };
        let pc = PeerConnection::new(pc_config).await?;
        let participant = Participant::spawn(
            participant_id.to_owned(),
            pc,
            Arc::clone(&self.mix_graph),
        )
        .await?;

        self.participants.insert(participant_id.to_owned(), participant);
        debug_evt!(
            "conference::room",
            "participant_joined",
            "room={}, participant={participant_id}",
            self.id
        );
        self.emit_participant_joined(participant_id);
        Ok(())
    }

    /// Removes a participant and tears down their peer connection.
    pub async fn remove_participant(&mut self, participant_id: &str) -> Result<(), ConferenceError> {
        self.remove_participant_inner(participant_id, true).await
    }

    /// Removes a participant with an optional kick reason.
    pub async fn kick_participant(
        &mut self,
        participant_id: &str,
        reason: Option<&str>,
    ) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::room",
            "kick_participant",
            "room={}, participant={participant_id}, reason={reason:?}",
            self.id
        );
        self.remove_participant_inner(participant_id, false).await?;
        debug_evt!(
            "conference::room",
            "participant_kicked",
            "room={}, participant={participant_id}, reason={reason:?}",
            self.id
        );
        self.emit_participant_kicked(participant_id, reason);
        Ok(())
    }

    /// Mutes a participant globally or for one listener.
    pub async fn mute_participant(
        &self,
        target_id: &str,
        scope: MuteScope,
        listener_id: Option<&str>,
    ) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::room",
            "mute_participant",
            "room={}, target={target_id}, scope={scope:?}, listener={listener_id:?}",
            self.id
        );
        self.ensure_participant_exists(target_id)?;
        if let Some(listener) = listener_id {
            self.ensure_participant_exists(listener)?;
        }
        self.mute_matrix.mute(target_id, scope, listener_id).await?;
        self.emit_participant_muted(target_id, scope, listener_id);
        Ok(())
    }

    /// Unmutes a participant globally or for one listener.
    pub async fn unmute_participant(
        &self,
        target_id: &str,
        scope: MuteScope,
        listener_id: Option<&str>,
    ) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::room",
            "unmute_participant",
            "room={}, target={target_id}, scope={scope:?}, listener={listener_id:?}",
            self.id
        );
        self.ensure_participant_exists(target_id)?;
        if let Some(listener) = listener_id {
            self.ensure_participant_exists(listener)?;
        }
        self.mute_matrix
            .unmute(target_id, scope, listener_id)
            .await
    }

    /// Returns participant summaries for admin/list APIs.
    pub fn list_participants(&self) -> Vec<ParticipantInfo> {
        self.participants
            .values()
            .map(|participant| ParticipantInfo {
                id: participant.id.clone(),
                connection_state: format!("{:?}", participant.connection_state()),
            })
            .collect()
    }

    /// Routes signaling DTOs to peer connection operations.
    pub async fn handle_signaling(
        &mut self,
        msg: SignalingMessage,
    ) -> Result<Vec<SignalingResponse>, ConferenceError> {
        debug_call!(
            "conference::room",
            "handle_signaling",
            "room={}, msg={msg:?}",
            self.id
        );

        match msg {
            SignalingMessage::Join {
                participant_id,
                room_id: _,
            } => {
                self.add_participant(&participant_id).await?;
                let participant = self
                    .participants
                    .get(&participant_id)
                    .ok_or_else(|| ConferenceError::internal("participant missing after join"))?;

                let offer = participant.peer_connection().create_offer(None).await?;
                participant
                    .peer_connection()
                    .set_local_description(offer)
                    .await?;
                participant.peer_connection().gathering_complete().await;

                let local_offer = participant
                    .peer_connection()
                    .local_description()
                    .await
                    .ok_or_else(|| ConferenceError::signaling_error("offer not available"))?;

                Ok(vec![SignalingResponse::Offer {
                    participant_id,
                    sdp: local_offer.sdp,
                }])
            }
            SignalingMessage::Answer { participant_id, sdp } => {
                let participant = self.participant_mut(&participant_id)?;
                participant
                    .peer_connection()
                    .set_remote_description(node_webrtc_rust_core::SessionDescription {
                        sdp_type: node_webrtc_rust_core::SdpType::Answer,
                        sdp,
                    })
                    .await?;
                Ok(Vec::new())
            }
            SignalingMessage::Offer { participant_id, sdp } => {
                let participant = self.participant_mut(&participant_id)?;
                participant
                    .peer_connection()
                    .set_remote_description(node_webrtc_rust_core::SessionDescription {
                        sdp_type: node_webrtc_rust_core::SdpType::Offer,
                        sdp,
                    })
                    .await?;

                let answer = participant.peer_connection().create_answer(None).await?;
                participant
                    .peer_connection()
                    .set_local_description(answer)
                    .await?;
                participant.peer_connection().gathering_complete().await;

                let local_answer = participant
                    .peer_connection()
                    .local_description()
                    .await
                    .ok_or_else(|| ConferenceError::signaling_error("answer not available"))?;

                Ok(vec![SignalingResponse::Answer {
                    participant_id,
                    sdp: local_answer.sdp,
                }])
            }
            SignalingMessage::IceCandidate {
                participant_id,
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => {
                let participant = self.participant_mut(&participant_id)?;
                participant
                    .peer_connection()
                    .add_ice_candidate(node_webrtc_rust_core::IceCandidate {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        username_fragment: None,
                    })
                    .await?;
                Ok(Vec::new())
            }
            SignalingMessage::Leave { participant_id } => {
                self.remove_participant(&participant_id).await?;
                Ok(Vec::new())
            }
        }
    }

    /// Injects a PCM frame into a participant input (testing and simulation).
    pub async fn inject_frame(
        &self,
        participant_id: &str,
        frame: Frame,
    ) -> Result<(), ConferenceError> {
        self.ensure_participant_exists(participant_id)?;
        let mut graph = self.mix_graph.lock().await;
        graph.push_frame(participant_id, frame);
        Ok(())
    }

    /// Renders the personalized mix for a participant.
    pub async fn render_output(&self, listener_id: &str) -> Result<Frame, ConferenceError> {
        self.ensure_participant_exists(listener_id)?;
        let graph = self.mix_graph.lock().await;
        Ok(graph.render_output(listener_id))
    }

    /// Closes all participants and clears room state.
    pub async fn close(&mut self) -> Result<(), ConferenceError> {
        debug_call!("conference::room", "close", "id={}", self.id);
        let ids: Vec<String> = self.participants.keys().cloned().collect();
        for id in ids {
            self.remove_participant(&id).await?;
        }
        Ok(())
    }

    async fn remove_participant_inner(
        &mut self,
        participant_id: &str,
        emit_left: bool,
    ) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::room",
            "remove_participant",
            "room={}, participant={participant_id}",
            self.id
        );

        let mut participant = self
            .participants
            .remove(participant_id)
            .ok_or_else(|| {
                ConferenceError::participant_not_found(format!(
                    "participant {participant_id} not in room {}",
                    self.id
                ))
            })?;

        participant.shutdown(&self.mix_graph).await?;
        if emit_left {
            debug_evt!(
                "conference::room",
                "participant_left",
                "room={}, participant={participant_id}",
                self.id
            );
            self.emit_participant_left(participant_id);
        }
        Ok(())
    }

    fn ensure_participant_exists(&self, participant_id: &str) -> Result<(), ConferenceError> {
        if self.participants.contains_key(participant_id) {
            Ok(())
        } else {
            Err(ConferenceError::participant_not_found(format!(
                "participant {participant_id} not in room {}",
                self.id
            )))
        }
    }

    fn participant_mut(&mut self, participant_id: &str) -> Result<&mut Participant, ConferenceError> {
        self.participants.get_mut(participant_id).ok_or_else(|| {
            ConferenceError::participant_not_found(format!(
                "participant {participant_id} not in room {}",
                self.id
            ))
        })
    }

    fn emit_participant_joined(&self, participant_id: &str) {
        if let Some(senders) = &self.event_senders {
            let _ = senders.participant_joined.send(ParticipantJoined {
                participant_id: participant_id.to_owned(),
            });
        }
    }

    fn emit_participant_left(&self, participant_id: &str) {
        if let Some(senders) = &self.event_senders {
            let _ = senders.participant_left.send(ParticipantLeft {
                participant_id: participant_id.to_owned(),
            });
        }
    }

    fn emit_participant_kicked(&self, participant_id: &str, reason: Option<&str>) {
        if let Some(senders) = &self.event_senders {
            let _ = senders.participant_kicked.send(ParticipantKicked {
                participant_id: participant_id.to_owned(),
                reason: reason.map(str::to_owned),
            });
        }
    }

    fn emit_participant_muted(&self, target_id: &str, scope: MuteScope, listener_id: Option<&str>) {
        if let Some(senders) = &self.event_senders {
            let _ = senders.participant_muted.send(ParticipantMuted {
                target_id: target_id.to_owned(),
                scope,
                listener_id: listener_id.map(str::to_owned),
            });
        }
    }

    fn emit_mixing_enabled_changed(&self, enabled: bool) {
        if let Some(senders) = &self.event_senders {
            let _ = senders
                .mixing_enabled_changed
                .send(MixingEnabledChanged { enabled });
        }
    }
}
