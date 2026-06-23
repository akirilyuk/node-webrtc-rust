import { EventEmitter } from 'events'

import type { JsRTCDataChannel as NativeDataChannel } from '@node-webrtc-rust/bindings'

import { debugEvent, debugFn } from './debug'
import type { MessageEvent, RTCDataChannelInit, RTCDataChannelState, RTCErrorEvent } from './types'

type SendPayload = string | Buffer | ArrayBuffer | Uint8Array

/**
 * WebRTC data channel for peer-to-peer messaging.
 *
 * Created via {@link RTCPeerConnection.createDataChannel} or received through
 * {@link RTCPeerConnection.ondatachannel}.
 */
export class RTCDataChannel extends EventEmitter {
  protected native: NativeDataChannel | null = null
  /** Application-defined channel label. */
  readonly label: string
  /** Whether messages are delivered in send order. */
  readonly ordered: boolean
  /** Sub-protocol negotiated for this channel. */
  readonly protocol: string
  /** Negotiated channel id, or null when assigned by the engine. */
  readonly id: number | null
  /** Current channel lifecycle state. */
  readyState: RTCDataChannelState = 'connecting'
  /** Bytes queued for transmission (not yet sent). */
  bufferedAmount = 0
  /** How binary messages are exposed in {@link onmessage}. */
  binaryType: 'arraybuffer' | 'blob' = 'arraybuffer'
  /** Threshold for {@link onbufferedamountlow}. */
  private _bufferedAmountLowThreshold = 0

  /** Fired when the channel is open and ready to send. */
  onopen: ((event: Event) => void) | null = null
  /** Fired when a message arrives from the remote peer. */
  onmessage: ((event: MessageEvent) => void) | null = null
  /** Fired when the channel closes. */
  onclose: ((event: Event) => void) | null = null
  /** Fired on send or transport errors. */
  onerror: ((event: RTCErrorEvent) => void) | null = null
  /** Fired when {@link bufferedAmount} drops to or below {@link bufferedAmountLowThreshold}. */
  onbufferedamountlow: ((event: Event) => void) | null = null

  private readonly pendingSends: SendPayload[] = []

  constructor(native: NativeDataChannel, init?: RTCDataChannelInit) {
    super()
    this.native = native
    this.label = native.label
    this.id = native.id
    this.readyState = native.readyState as RTCDataChannelState
    this.ordered = init?.ordered ?? true
    this.protocol = init?.protocol ?? ''
    this.attachNative(native)
  }

  /** Wraps a native channel that resolves asynchronously (outgoing channels). */
  static fromNativePromise(
    nativePromise: Promise<NativeDataChannel>,
    label: string,
    init?: RTCDataChannelInit,
  ): RTCDataChannel {
    const channel = new RTCDataChannel(createDeferredNative(label, init), init)
    channel.native = null
    void nativePromise
      .then((native) => {
        channel.native = native
        channel.readyState = native.readyState as RTCDataChannelState
        channel.attachNative(native)
        for (const payload of channel.pendingSends.splice(0)) {
          channel.send(payload)
        }
      })
      .catch((error: unknown) => {
        const event = createErrorEvent(error)
        channel.onerror?.(event)
        channel.emit('error', event)
      })
    return channel
  }

  /**
   * Sends a string or binary payload.
   * Messages sent before the native channel is ready are queued and flushed on open.
   */
  send(data: SendPayload): void {
    debugFn('sdk::RTCDataChannel', 'send', `label=${this.label}`)
    if (this.readyState !== 'open') {
      debugEvent(
        'sdk::RTCDataChannel',
        'send-dropped',
        `label=${this.label}, state=${this.readyState}`,
      )
      return
    }
    const byteLength = payloadByteLength(data)
    if (!this.native) {
      this.pendingSends.push(data)
      this.bufferedAmount += byteLength
      return
    }
    this.bufferedAmount += byteLength
    const payload = toNativeSendPayload(data)
    void this.native
      .send(payload)
      .then(() => this.refreshBufferedAmount())
      .catch((error: unknown) => this.emitError(error))
  }

  /** Closes the channel locally. */
  close(): void {
    debugFn('sdk::RTCDataChannel', 'close', `label=${this.label}`)
    if (this.native) {
      void swallowNativeClose(this.native.close(), 'sdk::RTCDataChannel', this.label, (error) =>
        this.emitError(error),
      )
    }
    this.readyState = 'closed'
  }

  /** Threshold at which {@link onbufferedamountlow} fires. */
  get bufferedAmountLowThreshold(): number {
    return this._bufferedAmountLowThreshold
  }

  set bufferedAmountLowThreshold(value: number) {
    this._bufferedAmountLowThreshold = value
    this.native?.setBufferedAmountLowThreshold(value)
  }

  protected attachNative(native: NativeDataChannel): void {
    native.setBufferedAmountLowThreshold(this._bufferedAmountLowThreshold)

    native.setOnOpen((_err) => {
      debugEvent('sdk::RTCDataChannel', 'open', `label=${this.label}`)
      this.readyState = 'open'
      const event = new Event('open')
      this.onopen?.(event)
      this.emit('open', event)
      this.refreshBufferedAmount()
    })

    native.setOnMessage((_err, data) => {
      if (data === undefined) return
      debugEvent(
        'sdk::RTCDataChannel',
        'message',
        `label=${this.label}, type=${typeof data === 'string' ? 'string' : 'binary'}`,
      )
      const message: MessageEvent =
        typeof data === 'string' ? { data } : { data: toMessageBinaryData(data, this.binaryType) }
      this.onmessage?.(message)
      this.emit('message', message)
    })

    native.setOnClose((_err) => {
      debugEvent('sdk::RTCDataChannel', 'close', `label=${this.label}`)
      this.readyState = 'closed'
      const event = new Event('close')
      this.onclose?.(event)
      this.emit('close', event)
    })

    native.setOnError((_err, message) => {
      this.emitError(
        typeof message === 'string' ? message : String(_err ?? 'unknown error'),
      )
    })

    native.setOnBufferedAmountLow((_err) => {
      debugEvent('sdk::RTCDataChannel', 'bufferedamountlow', `label=${this.label}`)
      void this.refreshBufferedAmount().then(() => {
        const event = new Event('bufferedamountlow')
        this.onbufferedamountlow?.(event)
        this.emit('bufferedamountlow', event)
      })
    })
  }

  private async refreshBufferedAmount(): Promise<void> {
    if (!this.native) return
    this.bufferedAmount = await this.native.bufferedAmount()
  }

  private emitError(error: unknown): void {
    const event = createErrorEvent(error)
    this.onerror?.(event)
    // Node throws ERR_UNHANDLED_ERROR when 'error' is emitted with no listeners.
    if (this.listenerCount('error') > 0) {
      this.emit('error', event)
      return
    }
    debugEvent(
      'sdk::RTCDataChannel',
      'error',
      `label=${this.label}, message=${event.message}`,
    )
  }
}

function toNativeSendPayload(data: SendPayload): string | Buffer {
  if (typeof data === 'string') {
    return data
  }
  if (Buffer.isBuffer(data)) {
    return data
  }
  if (data instanceof ArrayBuffer) {
    return Buffer.from(new Uint8Array(data))
  }
  return Buffer.from(data.buffer, data.byteOffset, data.byteLength)
}

function toMessageBinaryData(
  data: Buffer,
  binaryType: 'arraybuffer' | 'blob',
): Buffer | ArrayBuffer {
  if (binaryType === 'arraybuffer') {
    return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer
  }
  return data
}

function payloadByteLength(data: SendPayload): number {
  if (typeof data === 'string') {
    return Buffer.byteLength(data)
  }
  if (Buffer.isBuffer(data)) {
    return data.length
  }
  if (data instanceof ArrayBuffer) {
    return data.byteLength
  }
  return data.byteLength
}

function createErrorEvent(error: unknown): RTCErrorEvent {
  return {
    type: 'error',
    message: error instanceof Error ? error.message : String(error),
  }
}

function swallowNativeClose(
  closePromise: Promise<void>,
  scope: string,
  label: string,
  onError?: (error: unknown) => void,
): Promise<void> {
  return closePromise.catch((error: unknown) => {
    debugEvent(scope, 'close-error', `label=${label}, message=${formatErrorMessage(error)}`)
    onError?.(error)
  })
}

function formatErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function createDeferredNative(label: string, init?: RTCDataChannelInit): NativeDataChannel {
  return {
    label,
    id: init?.negotiated ?? 0,
    readyState: 'connecting',
    bufferedAmount: async () => 0,
    send: async () => undefined,
    close: async () => undefined,
    setBufferedAmountLowThreshold: () => undefined,
    setOnOpen: () => undefined,
    setOnMessage: () => undefined,
    setOnClose: () => undefined,
    setOnError: () => undefined,
    setOnBufferedAmountLow: () => undefined,
  }
}

export type { RTCDataChannelInit } from './types'
