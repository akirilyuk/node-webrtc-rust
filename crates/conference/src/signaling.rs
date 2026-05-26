//! Signaling message DTOs and JSON parsing.

use serde::{Deserialize, Serialize};

use crate::error::ConferenceError;

/// Inbound signaling messages from the Node bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SignalingMessage {
    Join {
        #[serde(rename = "participantId")]
        participant_id: String,
        #[serde(default, rename = "roomId")]
        room_id: Option<String>,
    },
    Offer {
        #[serde(rename = "participantId")]
        participant_id: String,
        sdp: String,
    },
    Answer {
        #[serde(rename = "participantId")]
        participant_id: String,
        sdp: String,
    },
    IceCandidate {
        #[serde(rename = "participantId")]
        participant_id: String,
        candidate: String,
        #[serde(default, rename = "sdpMid")]
        sdp_mid: Option<String>,
        #[serde(default, rename = "sdpMLineIndex")]
        sdp_mline_index: Option<u16>,
    },
    Leave {
        #[serde(rename = "participantId")]
        participant_id: String,
    },
}

/// Outbound signaling responses produced by the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SignalingResponse {
    Offer {
        #[serde(rename = "participantId")]
        participant_id: String,
        sdp: String,
    },
    Answer {
        #[serde(rename = "participantId")]
        participant_id: String,
        sdp: String,
    },
}

impl SignalingMessage {
    /// Parses a JSON signaling payload.
    pub fn from_json(json: &str) -> Result<Self, ConferenceError> {
        serde_json::from_str(json)
            .map_err(|err| ConferenceError::signaling_error(err.to_string()))
    }
}

impl SignalingResponse {
    /// Serializes a response to JSON for the Node bridge.
    pub fn to_json(&self) -> Result<String, ConferenceError> {
        serde_json::to_string(self)
            .map_err(|err| ConferenceError::signaling_error(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_join_message() {
        let msg = SignalingMessage::from_json(
            r#"{"type":"join","participantId":"alice","roomId":"room-1"}"#,
        )
        .unwrap();
        assert_eq!(
            msg,
            SignalingMessage::Join {
                participant_id: "alice".into(),
                room_id: Some("room-1".into()),
            }
        );
    }

    #[test]
    fn serialize_offer_response() {
        let response = SignalingResponse::Offer {
            participant_id: "alice".into(),
            sdp: "v=0".into(),
        };
        let json = response.to_json().unwrap();
        assert!(json.contains("\"type\":\"offer\""));
        assert!(json.contains("\"participantId\":\"alice\""));
    }
}
