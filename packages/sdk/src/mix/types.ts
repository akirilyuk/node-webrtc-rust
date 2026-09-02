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

export const MIX_PLACEMENT = {
  Center: 'center',
  Left: 'left',
  Right: 'right',
  Front: 'front',
  Behind: 'behind',
  Below: 'below',
  Above: 'above',
} as const satisfies Record<string, MixPlacement>
