import type { ClientPose, MixPlacement } from '@node-webrtc-rust/sdk/mix'

/** Named placement or world pose for clip play / TTS panning (mutually exclusive). */
export type AudioPosition =
  | { placement: MixPlacement; pose?: never }
  | { pose: ClientPose; placement?: never }

export function assertAudioPositionExclusive(position: AudioPosition): void {
  if (position.placement != null && position.pose != null) {
    throw new Error('AudioPosition: placement and pose are mutually exclusive')
  }
}
