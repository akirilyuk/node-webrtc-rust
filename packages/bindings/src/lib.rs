#![deny(clippy::all)]

mod conference;
mod config;
mod data_channel;
mod events;
mod media;
mod peer_connection;
mod rtp_receiver;
mod rtp_sender;
mod rtp_transceiver;
mod runtime;
mod speech;

use napi_derive::napi;

pub use conference::{
    JsConferenceRoom, JsConferenceServer, JsIceServer, JsMixingEnabledChangedEvent, JsMuteOptions,
    JsMuteScope, JsParticipantEvent, JsParticipantInfo, JsParticipantKickedEvent,
    JsParticipantMutedEvent, JsRoomErrorEvent, JsRoomOptions,
};
pub use config::{
    JsRTCAnswerOptions, JsRTCIceCandidate, JsRTCIceServer, JsRTCConfiguration, JsRTCOfferOptions,
    JsRTCSessionDescription,
};
pub use data_channel::{JsRTCDataChannel, JsRTCDataChannelInit};
pub use media::{JsLocalAudioTrack, JsMediaStream, JsMediaStreamTrack};
pub use peer_connection::JsPeerConnection;
pub use rtp_receiver::JsRtpReceiver;
pub use rtp_sender::JsRtpSender;
pub use rtp_transceiver::{JsRTCRtpTransceiverInit, JsRtpTransceiver};
pub use speech::{
    JsBargeInConfig, JsEventDeliveryMode, JsSpeechEvent, JsSpeechEventType, JsSttConfig,
    JsSttVendor, JsTtsConfig, JsTtsVendor, JsVadConfig, JsVadSampleRate, JsVoiceAgent,
    JsVoiceAgentConfig,
};

#[napi]
pub fn version() -> String {
    format!(
        "bindings={} core={} mixer={} conference={} speech={}",
        env!("CARGO_PKG_VERSION"),
        node_webrtc_rust_core::version(),
        node_webrtc_rust_mixer::version(),
        node_webrtc_rust_conference::version(),
        node_webrtc_rust_speech::version(),
    )
}
