/**
 * ICE transport policy for {@link RTCPeerConnection}.
 *
 * Production multi-node runners sit behind Cilium masquerade: host/srflx pairs
 * fail (UFW + SNAT) and webrtc-rs never CreatePermissions on the TURN
 * allocation, so relay↔relay never nominates. Set `WEBRTC_ICE_TRANSPORT_POLICY=relay`
 * on those runners so ICE only gathers/checks relay. Browsers stay `all`.
 */

export type IceTransportPolicy = 'all' | 'relay'

export function parseIceTransportPolicy(
  raw: string | undefined | null,
): IceTransportPolicy | undefined {
  const value = raw?.trim().toLowerCase()
  if (value === 'relay' || value === 'all') {
    return value
  }
  return undefined
}

/** Options override, then `WEBRTC_ICE_TRANSPORT_POLICY`, then `all`. */
export function resolveIceTransportPolicy(
  explicit?: IceTransportPolicy | string | null,
): IceTransportPolicy {
  return (
    parseIceTransportPolicy(explicit) ??
    parseIceTransportPolicy(process.env.WEBRTC_ICE_TRANSPORT_POLICY) ??
    'all'
  )
}
