use node_webrtc_rust_speech::error::SpeechError;

pub(crate) const VENDOR: &str = "mistral";

pub(crate) fn vendor_error(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: VENDOR.into(),
        message: message.into(),
    }
}
