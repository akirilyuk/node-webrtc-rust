#!/usr/bin/env node
/** Fails if @node-webrtc-rust/sdk ESM entry cannot load under Node. */
import { RTCPeerConnection } from '../packages/sdk/dist/esm/index.js'
import { VoiceAgent } from '../packages/sdk/dist/esm/voice/index.js'

if (typeof RTCPeerConnection !== 'function') {
  throw new Error('RTCPeerConnection export missing')
}
if (typeof VoiceAgent !== 'function') {
  throw new Error('VoiceAgent export missing')
}

console.log('smoke-esm-sdk-import ok')
