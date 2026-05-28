//! SDP offer/answer option types (W3C-aligned subset).

/// Options for [`crate::PeerConnection::create_offer`].
#[derive(Debug, Clone, Default)]
pub struct OfferOptions {
    /// When true, ICE credentials are regenerated (W3C `iceRestart`).
    pub ice_restart: bool,
    /// Opus voice-activity detection hint for the offer.
    pub voice_activity_detection: bool,
    /// When true, ensure a recv-only audio transceiver exists before creating the offer.
    pub offer_to_receive_audio: bool,
    /// When true, ensure a recv-only video transceiver (not supported — returns error).
    pub offer_to_receive_video: bool,
}

/// Options for [`crate::PeerConnection::create_answer`].
#[derive(Debug, Clone, Default)]
pub struct AnswerOptions {
    /// Opus voice-activity detection hint for the answer.
    pub voice_activity_detection: bool,
}
