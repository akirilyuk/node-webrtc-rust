//! JavaScript type conversions for mix graph APIs.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_mixer::{ClientPose, DistanceParams, MixPlacement, Quat, SpatialError, Vec3};

fn spatial_err(err: SpatialError) -> Error {
    match err {
        SpatialError::Invalid => Error::from_reason("invalid spatial value (NaN or zero quaternion)"),
    }
}

/// Named listener-relative placement when positional mixing is off.
#[napi(string_enum)]
#[derive(Debug)]
pub enum JsMixPlacement {
    #[napi(value = "center")]
    Center,
    #[napi(value = "left")]
    Left,
    #[napi(value = "right")]
    Right,
    #[napi(value = "front")]
    Front,
    #[napi(value = "behind")]
    Behind,
    #[napi(value = "below")]
    Below,
    #[napi(value = "above")]
    Above,
}

impl From<JsMixPlacement> for MixPlacement {
    fn from(value: JsMixPlacement) -> Self {
        match value {
            JsMixPlacement::Center => MixPlacement::Center,
            JsMixPlacement::Left => MixPlacement::Left,
            JsMixPlacement::Right => MixPlacement::Right,
            JsMixPlacement::Front => MixPlacement::Front,
            JsMixPlacement::Behind => MixPlacement::Behind,
            JsMixPlacement::Below => MixPlacement::Below,
            JsMixPlacement::Above => MixPlacement::Above,
        }
    }
}

impl From<MixPlacement> for JsMixPlacement {
    fn from(value: MixPlacement) -> Self {
        match value {
            MixPlacement::Center => JsMixPlacement::Center,
            MixPlacement::Left => JsMixPlacement::Left,
            MixPlacement::Right => JsMixPlacement::Right,
            MixPlacement::Front => JsMixPlacement::Front,
            MixPlacement::Behind => JsMixPlacement::Behind,
            MixPlacement::Below => JsMixPlacement::Below,
            MixPlacement::Above => JsMixPlacement::Above,
        }
    }
}

/// 3D vector for positional mixing.
#[napi(object)]
#[derive(Debug, Clone, Copy)]
pub struct JsVec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl TryFrom<JsVec3> for Vec3 {
    type Error = Error;

    fn try_from(value: JsVec3) -> Result<Self> {
        Vec3::try_new(value.x as f32, value.y as f32, value.z as f32).map_err(spatial_err)
    }
}

impl From<Vec3> for JsVec3 {
    fn from(value: Vec3) -> Self {
        Self {
            x: value.x as f64,
            y: value.y as f64,
            z: value.z as f64,
        }
    }
}

/// Unit quaternion for positional mixing.
#[napi(object)]
#[derive(Debug, Clone, Copy)]
pub struct JsQuat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl TryFrom<JsQuat> for Quat {
    type Error = Error;

    fn try_from(value: JsQuat) -> Result<Self> {
        Quat::try_new(
            value.x as f32,
            value.y as f32,
            value.z as f32,
            value.w as f32,
        )
        .map_err(spatial_err)
    }
}

impl From<Quat> for JsQuat {
    fn from(value: Quat) -> Self {
        Self {
            x: value.x as f64,
            y: value.y as f64,
            z: value.z as f64,
            w: value.w as f64,
        }
    }
}

/// World-space pose for a mix participant.
#[napi(object)]
#[derive(Debug, Clone, Copy)]
pub struct JsClientPose {
    pub position: JsVec3,
    pub orientation: JsQuat,
}

impl TryFrom<JsClientPose> for ClientPose {
    type Error = Error;

    fn try_from(value: JsClientPose) -> Result<Self> {
        ClientPose::try_new(value.position.try_into()?, value.orientation.try_into()?)
            .map_err(spatial_err)
    }
}

impl From<ClientPose> for JsClientPose {
    fn from(value: ClientPose) -> Self {
        Self {
            position: value.position.into(),
            orientation: value.orientation.into(),
        }
    }
}

/// Distance attenuation parameters for positional panning.
#[napi(object)]
#[derive(Debug, Clone, Copy)]
pub struct JsDistanceParams {
    pub reference_distance: f64,
    pub max_distance: f64,
    pub rolloff: f64,
}

impl From<JsDistanceParams> for DistanceParams {
    fn from(value: JsDistanceParams) -> Self {
        Self {
            reference_distance: value.reference_distance as f32,
            max_distance: value.max_distance as f32,
            rolloff: value.rolloff as f32,
        }
    }
}

impl From<DistanceParams> for JsDistanceParams {
    fn from(value: DistanceParams) -> Self {
        Self {
            reference_distance: value.reference_distance as f64,
            max_distance: value.max_distance as f64,
            rolloff: value.rolloff as f64,
        }
    }
}
