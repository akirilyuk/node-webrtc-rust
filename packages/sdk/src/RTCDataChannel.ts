import { EventEmitter } from 'events'

import type { JsRTCDataChannel as NativeDataChannel } from '@node-webrtc-rust/bindings'

import type { MessageEvent, RTCDataChannelInit, RTCDataChannelState, RTCErrorEvent } from './types'

type SendPayload = string | Buffer | ArrayBuffer | Uint8Array

export class RTCDataChannel extends EventEmitter {
  protected native: NativeDataChannel | null = null
  readonly label: string
  readonly ordered: boolean
  readonly protocol: string
  readonly id: number | null
  readyState: RTCDataChannelState = 'connecting'
  bufferedAmount = 0
  binaryType: 'arraybuffer' | 'blob' = 'arraybuffer'
  bufferedAmountLowThreshold = 0

  onopen: ((event: Event) => void) | null = null
  onmessage: ((event: MessageEvent) => void) | null = null
  onclose: ((event: Event) => void) | null = null
  onerror: ((event: RTCErrorEvent) => void) | null = null
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

  send(data: SendPayload): void {
    if (!this.native) {
      this.pendingSends.push(data)
      return
    }
    const payload =
      typeof data === 'string'
        ? data
        : Buffer.isBuffer(data)
          ? data
          : data instanceof ArrayBuffer
            ? Buffer.from(new Uint8Array(data))
            : Buffer.from(data)
    void this.native.send(payload).catch((error: unknown) => this.emitError(error))
  }

  close(): void {
    if (this.native) {
      void this.native.close()
    }
    this.readyState = 'closed'
  }

  protected attachNative(native: NativeDataChannel): void {
    native.setOnOpen(() => {
      this.readyState = 'open'
      const event = new Event('open')
      this.onopen?.(event)
      this.emit('open', event)
    })

    native.setOnMessage((data) => {
      const message: MessageEvent =
        typeof data === 'string' ? { data } : { data: Buffer.from(data) }
      this.onmessage?.(message)
      this.emit('message', message)
    })

    native.setOnClose(() => {
      this.readyState = 'closed'
      const event = new Event('close')
      this.onclose?.(event)
      this.emit('close', event)
    })

    native.setOnError((message) => {
      const event: RTCErrorEvent = { type: 'error', message }
      this.onerror?.(event)
      this.emit('error', event)
    })
  }

  private emitError(error: unknown): void {
    const event = createErrorEvent(error)
    this.onerror?.(event)
    this.emit('error', event)
  }
}

function createErrorEvent(error: unknown): RTCErrorEvent {
  return {
    type: 'error',
    message: error instanceof Error ? error.message : String(error),
  }
}

function createDeferredNative(label: string, init?: RTCDataChannelInit): NativeDataChannel {
  return {
    label,
    id: init?.negotiated ?? 0,
    readyState: 'connecting',
    bufferedAmount: async () => 0,
    send: async () => undefined,
    close: async () => undefined,
    setOnOpen: () => undefined,
    setOnMessage: () => undefined,
    setOnClose: () => undefined,
    setOnError: () => undefined,
  }
}

export type { RTCDataChannelInit } from './types'
