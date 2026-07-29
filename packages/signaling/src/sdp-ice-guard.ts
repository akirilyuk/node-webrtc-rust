/**
 * Re-export SDP ICE credential helpers from sdk (canonical implementation).
 * Kept for stable `@node-webrtc-rust/signaling` import paths.
 */
export {
  assertSdpHasIceCredentials,
  describeSdpIce,
  type SdpIceKind,
  type SdpIceMeta,
} from '@node-webrtc-rust/sdk'
