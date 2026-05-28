//! JavaScript configuration and SDP/ICE type conversions.

use napi::bindgen_prelude::{Env, FromNapiValue, Result, ToNapiValue};
use napi::JsUnknown;
use napi_derive::napi;
use node_webrtc_rust_core::{
    AnswerOptions, IceCandidate, IceServer, IceTransportPolicy, OfferOptions, PeerConnectionConfig,
    SdpType, SessionDescription, set_debug_enabled,
};

/// ICE server configuration exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsRTCIceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
    pub credential_type: Option<String>,
}

impl From<JsRTCIceServer> for IceServer {
    fn from(value: JsRTCIceServer) -> Self {
        let credential_type = match value.credential_type.as_deref() {
            Some("oauth") => node_webrtc_rust_core::IceCredentialType::Oauth,
            _ => node_webrtc_rust_core::IceCredentialType::Password,
        };

        Self {
            urls: value.urls,
            username: value.username,
            credential: value.credential,
            credential_type,
        }
    }
}

impl From<IceServer> for JsRTCIceServer {
    fn from(value: IceServer) -> Self {
        Self {
            urls: value.urls,
            username: value.username,
            credential: value.credential,
            credential_type: Some(match value.credential_type {
                node_webrtc_rust_core::IceCredentialType::Oauth => "oauth".to_string(),
                node_webrtc_rust_core::IceCredentialType::Password => "password".to_string(),
            }),
        }
    }
}

/// Peer connection configuration exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRTCConfiguration {
    pub ice_servers: Option<Vec<JsRTCIceServer>>,
    pub ice_transport_policy: Option<String>,
    pub debug: Option<bool>,
}

impl From<JsRTCConfiguration> for PeerConnectionConfig {
    fn from(value: JsRTCConfiguration) -> Self {
        let ice_transport_policy = match value.ice_transport_policy.as_deref() {
            Some("relay") => IceTransportPolicy::Relay,
            _ => IceTransportPolicy::All,
        };

        if let Some(debug) = value.debug {
            set_debug_enabled(debug);
        }

        Self {
            ice_servers: value
                .ice_servers
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            ice_transport_policy,
            debug: value.debug,
        }
    }
}

impl From<PeerConnectionConfig> for JsRTCConfiguration {
    fn from(value: PeerConnectionConfig) -> Self {
        Self {
            ice_servers: Some(
                value
                    .ice_servers
                    .into_iter()
                    .map(JsRTCIceServer::from)
                    .collect(),
            ),
            ice_transport_policy: Some(match value.ice_transport_policy {
                IceTransportPolicy::Relay => "relay".to_string(),
                IceTransportPolicy::All => "all".to_string(),
            }),
            debug: value.debug,
        }
    }
}

/// Offer options (W3C `RTCOfferOptions` subset).
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRTCOfferOptions {
    pub ice_restart: Option<bool>,
    pub voice_activity_detection: Option<bool>,
    pub offer_to_receive_audio: Option<bool>,
    pub offer_to_receive_video: Option<bool>,
}

pub(crate) fn offer_options_from_js(value: Option<JsRTCOfferOptions>) -> OfferOptions {
    let value = value.unwrap_or_default();
    OfferOptions {
        ice_restart: value.ice_restart.unwrap_or(false),
        voice_activity_detection: value.voice_activity_detection.unwrap_or(true),
        offer_to_receive_audio: value.offer_to_receive_audio.unwrap_or(false),
        offer_to_receive_video: value.offer_to_receive_video.unwrap_or(false),
    }
}

/// Answer options (W3C `RTCAnswerOptions` subset).
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRTCAnswerOptions {
    pub voice_activity_detection: Option<bool>,
}

pub(crate) fn answer_options_from_js(value: Option<JsRTCAnswerOptions>) -> AnswerOptions {
    let value = value.unwrap_or_default();
    AnswerOptions {
        voice_activity_detection: value.voice_activity_detection.unwrap_or(true),
    }
}

/// Session description exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsRTCSessionDescription {
    pub r#type: String,
    pub sdp: String,
}

impl From<SessionDescription> for JsRTCSessionDescription {
    fn from(value: SessionDescription) -> Self {
        Self {
            r#type: sdp_type_to_string(value.sdp_type),
            sdp: value.sdp,
        }
    }
}

impl TryFrom<JsRTCSessionDescription> for SessionDescription {
    type Error = napi::Error;

    fn try_from(value: JsRTCSessionDescription) -> napi::Result<Self> {
        Ok(Self {
            sdp_type: sdp_type_from_string(&value.r#type)?,
            sdp: value.sdp,
        })
    }
}

/// ICE candidate exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRTCIceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_m_line_index: Option<u16>,
    pub username_fragment: Option<String>,
}

impl From<IceCandidate> for JsRTCIceCandidate {
    fn from(value: IceCandidate) -> Self {
        Self {
            candidate: value.candidate,
            sdp_mid: value.sdp_mid,
            sdp_m_line_index: value.sdp_mline_index,
            username_fragment: value.username_fragment,
        }
    }
}

impl From<JsRTCIceCandidate> for IceCandidate {
    fn from(value: JsRTCIceCandidate) -> Self {
        Self {
            candidate: value.candidate,
            sdp_mid: value.sdp_mid,
            sdp_mline_index: value.sdp_m_line_index,
            username_fragment: value.username_fragment,
        }
    }
}

fn sdp_type_to_string(sdp_type: SdpType) -> String {
    match sdp_type {
        SdpType::Offer => "offer".to_string(),
        SdpType::Answer => "answer".to_string(),
        SdpType::ProvisionalAnswer => "pranswer".to_string(),
        SdpType::Rollback => "rollback".to_string(),
    }
}

fn sdp_type_from_string(value: &str) -> napi::Result<SdpType> {
    match value {
        "offer" => Ok(SdpType::Offer),
        "answer" => Ok(SdpType::Answer),
        "pranswer" => Ok(SdpType::ProvisionalAnswer),
        "rollback" => Ok(SdpType::Rollback),
        other => Err(napi::Error::from_reason(format!(
            "invalid session description type: {other}"
        ))),
    }
}

pub(crate) fn core_err(err: node_webrtc_rust_core::CoreError) -> napi::Error {
    napi::Error::from_reason(err.to_string())
}

pub(crate) fn to_js_unknown<T: ToNapiValue>(env: &Env, value: T) -> Result<JsUnknown> {
    unsafe {
        JsUnknown::from_napi_value(
            env.raw(),
            T::to_napi_value(env.raw(), value)?,
        )
    }
}
