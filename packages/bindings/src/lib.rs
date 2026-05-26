#![deny(clippy::all)]

mod config;
mod data_channel;
mod events;
mod media;
mod peer_connection;
mod runtime;

use napi_derive::napi;

pub use config::{
    JsRTCIceCandidate, JsRTCIceServer, JsRTCConfiguration, JsRTCSessionDescription,
};
pub use data_channel::{JsRTCDataChannel, JsRTCDataChannelInit};
pub use media::{JsMediaStream, JsMediaStreamTrack};
pub use peer_connection::JsPeerConnection;

#[napi]
pub fn version() -> String {
    format!(
        "bindings={} core={} mixer={}",
        env!("CARGO_PKG_VERSION"),
        node_webrtc_rust_core::version(),
        node_webrtc_rust_mixer::version(),
    )
}
