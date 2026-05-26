#![deny(clippy::all)]

use napi_derive::napi;

#[napi]
pub fn version() -> String {
    format!(
        "bindings={} core={} mixer={}",
        env!("CARGO_PKG_VERSION"),
        node_webrtc_rust_core::version(),
        node_webrtc_rust_mixer::version(),
    )
}
