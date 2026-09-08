#![deny(clippy::all)]

mod conference;
mod config;
mod data_channel;
mod events;
mod media;
mod mixer;
mod peer_connection;
mod player;
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
    JsRTCAnswerOptions, JsRTCConfiguration, JsRTCIceCandidate, JsRTCIceServer, JsRTCOfferOptions,
    JsRTCSessionDescription,
};
pub use data_channel::{JsRTCDataChannel, JsRTCDataChannelInit};
pub use media::{JsLocalAudioTrack, JsMediaStream, JsMediaStreamTrack};
pub use mixer::{
    js_quat_identity, js_vec3_zero, JsClientPose, JsDistanceParams, JsMixGraph, JsMixPlacement,
    JsQuat, JsVec3,
};
pub use player::{
    get_clip_status, play_clip_from_bytes, play_clip_from_path, play_clip_progressive, stop_clip,
    take_clip_frame, JsClipPlayerStatus, JsClipStatus, JsGrowingClipWriter,
};
pub use peer_connection::JsPeerConnection;
pub use rtp_receiver::JsRtpReceiver;
pub use rtp_sender::JsRtpSender;
pub use rtp_transceiver::{JsRTCRtpTransceiverInit, JsRtpTransceiver};
pub use speech::{
    JsBargeInConfig, JsEventDeliveryMode, JsSessionAudioFormat, JsSessionFinalizeResult,
    JsSessionRecorder, JsSpeechEvent, JsSpeechEventType, JsSttConfig, JsSttVendor, JsTtsConfig,
    JsTtsVendor, JsVadConfig, JsVadSampleRate, JsVoiceAgent, JsVoiceAgentConfig,
};

#[napi]
pub fn version() -> String {
    format!(
        "bindings={} core={} mixer={} conference={} speech={} player={}",
        env!("CARGO_PKG_VERSION"),
        node_webrtc_rust_core::version(),
        node_webrtc_rust_mixer::version(),
        node_webrtc_rust_conference::version(),
        node_webrtc_rust_speech::version(),
        node_webrtc_rust_player::version(),
    )
}
