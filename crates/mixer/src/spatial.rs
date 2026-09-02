//! Spatial types and stereo pan math for positional mixing.
//!
//! Coordinate system: right-handed **Y-up**; listener looks down **−Z** after
//! inverse orientation. Panning is equal-power stereo from listener-local
//! azimuth plus distance attenuation (no HRTF).

use crate::frame::{Frame, FRAME_BYTES, SAMPLES_PER_FRAME};

/// Error returned when pose components are invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialError {
    /// NaN component or zero-length quaternion.
    Invalid,
}

/// 3D vector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    /// Creates a vector, rejecting NaN components.
    pub fn try_new(x: f32, y: f32, z: f32) -> Result<Self, SpatialError> {
        if x.is_nan() || y.is_nan() || z.is_nan() {
            return Err(SpatialError::Invalid);
        }
        Ok(Self { x, y, z })
    }

    /// Origin.
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }

    fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

/// Unit quaternion (normalized on construction).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    /// Creates a normalized quaternion, rejecting NaN and zero length.
    pub fn try_new(x: f32, y: f32, z: f32, w: f32) -> Result<Self, SpatialError> {
        if x.is_nan() || y.is_nan() || z.is_nan() || w.is_nan() {
            return Err(SpatialError::Invalid);
        }
        let len_sq = x * x + y * y + z * z + w * w;
        if len_sq < f32::EPSILON {
            return Err(SpatialError::Invalid);
        }
        let inv_len = len_sq.sqrt().recip();
        Ok(Self {
            x: x * inv_len,
            y: y * inv_len,
            z: z * inv_len,
            w: w * inv_len,
        })
    }

    /// Identity rotation (no rotation).
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// Rotates `v` by the inverse of this quaternion (listener → world).
    fn rotate_inverse(self, v: Vec3) -> Vec3 {
        // q^-1 * v * q  (optimized for unit quaternion)
        let qx = self.x;
        let qy = self.y;
        let qz = self.z;
        let qw = self.w;

        let tx = 2.0 * (qy * v.z - qz * v.y);
        let ty = 2.0 * (qz * v.x - qx * v.z);
        let tz = 2.0 * (qx * v.y - qy * v.x);

        Vec3 {
            x: v.x + qw * tx + (qy * tz - qz * ty),
            y: v.y + qw * ty + (qz * tx - qx * tz),
            z: v.z + qw * tz + (qx * ty - qy * tx),
        }
    }
}

/// World-space pose for a mix participant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientPose {
    pub position: Vec3,
    pub orientation: Quat,
}

impl ClientPose {
    /// Creates a pose from validated position and orientation.
    pub fn try_new(position: Vec3, orientation: Quat) -> Result<Self, SpatialError> {
        Ok(Self {
            position,
            orientation,
        })
    }

    /// Origin facing forward (−Z).
    pub fn center() -> Self {
        Self {
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
        }
    }
}

/// Named listener-relative placement when positional mixing is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MixPlacement {
    #[default]
    Center,
    Left,
    Right,
    Front,
    Behind,
    Below,
    Above,
}

impl MixPlacement {
    /// Listener-relative direction and distance-scale for this placement.
    ///
    /// `left` / `right` hard-pan; `front` = unity center; `behind` = center +
    /// attenuation; `above` / `below` = near-center with mild gain tilt.
    pub fn listener_relative(self) -> (Vec3, f32) {
        match self {
            Self::Center | Self::Front => (
                Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: -1.0,
                },
                1.0,
            ),
            Self::Left => (
                Vec3 {
                    x: -1.0,
                    y: 0.0,
                    z: 0.0,
                },
                1.0,
            ),
            Self::Right => (
                Vec3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
                1.0,
            ),
            Self::Behind => (
                Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                },
                0.35,
            ),
            Self::Above => (
                Vec3 {
                    x: 0.0,
                    y: 1.0,
                    z: -0.25,
                },
                0.85,
            ),
            Self::Below => (
                Vec3 {
                    x: 0.0,
                    y: -1.0,
                    z: -0.25,
                },
                0.85,
            ),
        }
    }
}

/// Distance attenuation parameters for positional panning.
///
/// Defaults: `reference_distance = 1`, `max_distance = 50`, `rolloff = 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistanceParams {
    /// Distance at which gain is unity.
    pub reference_distance: f32,
    /// Distance beyond which gain is zero.
    pub max_distance: f32,
    /// Rolloff factor (1 ≈ inverse-linear falloff).
    pub rolloff: f32,
}

impl Default for DistanceParams {
    fn default() -> Self {
        Self {
            reference_distance: 1.0,
            max_distance: 50.0,
            rolloff: 1.0,
        }
    }
}

impl DistanceParams {
    /// Gain from listener-local distance (0..1).
    pub fn gain_at_distance(self, distance: f32) -> f32 {
        if distance <= self.reference_distance {
            return 1.0;
        }
        if distance >= self.max_distance {
            return 0.0;
        }
        let range = self.max_distance - self.reference_distance;
        if range <= f32::EPSILON {
            return 0.0;
        }
        let t = (distance - self.reference_distance) / range;
        (1.0 - t * self.rolloff).clamp(0.0, 1.0)
    }
}

/// Left/right equal-power gains from listener-local direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanGains {
    pub left: f32,
    pub right: f32,
}

/// Computes stereo pan gains from listener-local direction and distance.
pub fn pan_gains_from_local(local: Vec3, distance_scale: f32, params: DistanceParams) -> PanGains {
    let distance = local.length();
    let dist_gain = params.gain_at_distance(distance) * distance_scale;

    // Azimuth: 0 = front (−Z), positive = right (+X).
    let azimuth = if distance < f32::EPSILON {
        0.0
    } else {
        local.x.atan2(-local.z)
    };

    // Map ±π/2 to hard pan.
    let pan = (azimuth / (std::f32::consts::FRAC_PI_2)).clamp(-1.0, 1.0);
    let t = (pan + 1.0) * 0.5;
    let l = (t * std::f32::consts::FRAC_PI_2).cos() * dist_gain;
    let r = (t * std::f32::consts::FRAC_PI_2).sin() * dist_gain;

    PanGains { left: l, right: r }
}

/// Pan gains when positional mixing is on (source vs listener pose).
pub fn pan_gains_positional(
    listener: ClientPose,
    source: ClientPose,
    params: DistanceParams,
) -> PanGains {
    let world_delta = source.position.sub(listener.position);
    let local = listener.orientation.rotate_inverse(world_delta);
    pan_gains_from_local(local, 1.0, params)
}

/// Pan gains from a named placement (positional off).
pub fn pan_gains_placement(placement: MixPlacement, params: DistanceParams) -> PanGains {
    match placement {
        MixPlacement::Center | MixPlacement::Front => PanGains {
            left: 1.0,
            right: 1.0,
        },
        other => {
            let (dir, scale) = other.listener_relative();
            pan_gains_from_local(dir, scale, params)
        }
    }
}

/// Applies per-channel gains to a stereo frame.
pub fn apply_pan_gains(frame: &Frame, gains: PanGains) -> Frame {
    if frame.pcm.len() != FRAME_BYTES {
        return frame.clone();
    }

    let mut pcm = frame.pcm.to_vec();
    for i in 0..SAMPLES_PER_FRAME / 2 {
        let base = i * 4;
        let l = i16::from_le_bytes([pcm[base], pcm[base + 1]]);
        let r = i16::from_le_bytes([pcm[base + 2], pcm[base + 3]]);

        let l_out = (f32::from(l) * gains.left).round() as i32;
        let r_out = (f32::from(r) * gains.right).round() as i32;

        pcm[base..base + 2].copy_from_slice(&(l_out as i16).to_le_bytes());
        pcm[base + 2..base + 4].copy_from_slice(&(r_out as i16).to_le_bytes());
    }

    Frame::new(bytes::Bytes::from(pcm), frame.timestamp_us)
}

/// Pans a frame using a named [`MixPlacement`] (TTS or default client placement).
pub fn pan_frame_with_placement(
    frame: &Frame,
    placement: MixPlacement,
    params: DistanceParams,
) -> Frame {
    apply_pan_gains(frame, pan_gains_placement(placement, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono_frame(amplitude: i16) -> Frame {
        let mut pcm = vec![0u8; FRAME_BYTES];
        for i in 0..SAMPLES_PER_FRAME / 2 {
            let base = i * 4;
            pcm[base..base + 2].copy_from_slice(&amplitude.to_le_bytes());
            pcm[base + 2..base + 4].copy_from_slice(&amplitude.to_le_bytes());
        }
        Frame::new(bytes::Bytes::from(pcm), None)
    }

    fn first_lr(frame: &Frame) -> (i16, i16) {
        (
            i16::from_le_bytes([frame.pcm[0], frame.pcm[1]]),
            i16::from_le_bytes([frame.pcm[2], frame.pcm[3]]),
        )
    }

    #[test]
    fn quat_rejects_nan_and_zero() {
        assert!(Quat::try_new(f32::NAN, 0.0, 0.0, 1.0).is_err());
        assert!(Quat::try_new(0.0, 0.0, 0.0, 0.0).is_err());
        let q = Quat::try_new(0.0, 0.0, 0.0, 2.0).unwrap();
        assert!((q.w - 1.0).abs() < 1e-5);
    }

    #[test]
    fn vec3_rejects_nan() {
        assert!(Vec3::try_new(f32::NAN, 0.0, 0.0).is_err());
    }

    #[test]
    fn source_to_listeners_right_is_louder_in_right() {
        let listener = ClientPose::center();
        let source = ClientPose {
            position: Vec3::try_new(2.0, 0.0, 0.0).unwrap(),
            orientation: Quat::IDENTITY,
        };
        let gains = pan_gains_positional(listener, source, DistanceParams::default());
        assert!(gains.right > gains.left);
    }

    #[test]
    fn placement_left_is_louder_in_left() {
        let gains = pan_gains_placement(MixPlacement::Left, DistanceParams::default());
        assert!(gains.left > gains.right);
    }

    #[test]
    fn placement_right_pans_tts_frame() {
        let frame = mono_frame(10_000);
        let panned =
            pan_frame_with_placement(&frame, MixPlacement::Right, DistanceParams::default());
        let (l, r) = first_lr(&panned);
        assert!(r > l);
        assert!(r > 0);
    }

    #[test]
    fn placement_behind_attenuates() {
        let front = pan_gains_placement(MixPlacement::Front, DistanceParams::default());
        let behind = pan_gains_placement(MixPlacement::Behind, DistanceParams::default());
        assert!(behind.left < front.left);
        assert!(behind.right < front.right);
    }
}
