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
 *
 * `played` counts frames popped by the media clock, not frames a listener received. A listener
 * whose renders run late receives those frames merged (`listenerCatchupCompressed`) or, beyond
 * 8 ticks behind, jumps over them (`listenerSkipped`). Both are summed over listeners, so a
 * source heard by two late listeners counts twice. Listener-level losses are visible here and
 * never silent: frames heard = `played - listenerSkipped - listenerCatchupCompressed` for a
 * single listener.
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
  /** Popped frames merged into one output frame for a listener that rendered late. */
  listenerCatchupCompressed: number
  /** Popped frames a listener jumped over after a render more than 8 ticks (160 ms) late. */
  listenerSkipped: number
  /** Frames currently queued. */
  depth: number
  /** Current adaptive target depth in frames (2 to 8, i.e. 40 to 160 ms). */
  target: number
}

/** Native mix pump counters (polled at 1 Hz). */
export interface NativePumpStats {
  /** TTS frames currently queued for the pump. */
  ttsQueueFrames: number
  /** Deepest TTS backlog (frames) since the previous stats read. */
  ttsQueueMaxDepthFrames: number
  /** Outbound frames written since the pump started. */
  framesWritten: number
  /** Due slots skipped during an active TTS stream. */
  slotsSkipped: number
  /** Times the pump clock was rebased after a long stall. */
  rebases: number
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
