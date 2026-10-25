/**
 * Whether the remote party asked to receive stereo Opus (RFC 7587 `stereo=1` on its fmtp).
 *
 * Returns `null` when the SDP is empty or carries no Opus payload type, `true` when any Opus
 * payload type has `stereo=1`, otherwise `false` (a mono decoder is likely, e.g. Chrome's default
 * answer `minptime=10;useinbandfec=1`). `sprop-stereo` describes what the party sends and is
 * deliberately ignored.
 */
export function opusStereoPreference(sdp: string | null | undefined): boolean | null {
  if (!sdp) return null
  const lines = sdp.split(/\r?\n/)
  const opusPts = new Set<string>()
  for (const line of lines) {
    const m = /^a=rtpmap:(\d+)\s+opus\//i.exec(line.trim())
    if (m) opusPts.add(m[1])
  }
  if (opusPts.size === 0) return null
  for (const line of lines) {
    const m = /^a=fmtp:(\d+)\s+(.*)$/.exec(line.trim())
    if (!m || !opusPts.has(m[1])) continue
    for (const param of m[2].split(';')) {
      const p = param.trim()
      const eq = p.indexOf('=')
      if (eq < 0) continue
      if (p.slice(0, eq).trim().toLowerCase() === 'stereo' && p.slice(eq + 1).trim() === '1') {
        return true
      }
    }
  }
  return false
}
