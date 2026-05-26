#![deny(clippy::all)]

mod conference_room;
mod conference_server;
mod events;
mod runtime;
mod types;

use napi_derive::napi;

pub use conference_room::JsConferenceRoom;
pub use conference_server::JsConferenceServer;
pub use types::{
    JsIceServer, JsMixingEnabledChangedEvent, JsMuteOptions, JsMuteScope, JsParticipantEvent,
    JsParticipantInfo, JsParticipantKickedEvent, JsParticipantMutedEvent, JsRoomErrorEvent,
    JsRoomOptions,
};

/// Returns version strings for the conference bindings and underlying crate.
#[napi]
pub fn version() -> String {
    format!(
        "conference-bindings={} conference={}",
        env!("CARGO_PKG_VERSION"),
        node_webrtc_rust_conference::version(),
    )
}
