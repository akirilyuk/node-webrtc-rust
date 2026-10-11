/** Named listener-relative placement when positional mixing is off. */
export type MixPlacement = 'center' | 'left' | 'right' | 'front' | 'behind' | 'below' | 'above'

/** 3D vector for positional mixing. */
export interface Vec3 {
  x: number
  y: number
  z: number
}

/** Unit quaternion for positional mixing. */
export interface Quat {
  x: number
  y: number
  z: number
  w: number
}

/** World-space pose for a mix participant. */
export interface ClientPose {
  position: Vec3
  orientation: Quat
}

/** Distance attenuation parameters for positional panning. */
export interface DistanceParams {
  referenceDistance: number
  maxDistance: number
  rolloff: number
}

/**
 * Per-source mix queue counters from `AudioMixGraph.mixStats`.
 *
 * Invariant: `pushed = played + compressed + overflowCompressed + depth`.
 */
export interface MixStats {
  /** Frames pushed into the source queue. */
  pushed: number
  /** Source frames played into the mix. */
  played: number
  /** Source frames absorbed by time-compression while the backlog was above target. */
  compressed: number
  /** Faded concealment frames played on underrun. */
  concealed: number
  /** Source frames absorbed by merging at the 1 s queue capacity (capacity signal). */
  overflowCompressed: number
  /** Inbound packets that failed to decode. */
  decodeErrors: number
  /** Frames currently queued. */
  depth: number
  /** Current adaptive target depth in frames (2 to 8, i.e. 40 to 160 ms). */
  target: number
}

export const MIX_PLACEMENT = {
  Center: 'center',
  Left: 'left',
  Right: 'right',
  Front: 'front',
  Behind: 'behind',
  Below: 'below',
  Above: 'above',
} as const satisfies Record<string, MixPlacement>
