import {
  jsQuatIdentity,
  jsVec3Zero,
  JsMixGraph,
  type JsClientPose,
  type JsDistanceParams,
  type JsMixPlacement,
} from '@node-webrtc-rust/bindings'

import type { ClientPose, DistanceParams, MixPlacement, Quat, Vec3 } from './types.js'

function toJsPlacement(placement: MixPlacement): JsMixPlacement {
  return placement as JsMixPlacement
}

function fromJsPlacement(placement: JsMixPlacement): MixPlacement {
  return placement as MixPlacement
}

function toJsPose(pose: ClientPose): JsClientPose {
  return {
    position: pose.position,
    orientation: pose.orientation,
  }
}

function fromJsPose(pose: JsClientPose): ClientPose {
  return {
    position: pose.position,
    orientation: pose.orientation,
  }
}

function toJsDistanceParams(params: DistanceParams): JsDistanceParams {
  return {
    referenceDistance: params.referenceDistance,
    maxDistance: params.maxDistance,
    rolloff: params.rolloff,
  }
}

function fromJsDistanceParams(params: JsDistanceParams): DistanceParams {
  return {
    referenceDistance: params.referenceDistance,
    maxDistance: params.maxDistance,
    rolloff: params.rolloff,
  }
}

/** Control handle for a conference mix graph (poses, groups, mutes, placements). */
export class AudioMixGraph {
  constructor(private readonly native: JsMixGraph = new JsMixGraph()) {}

  /** @internal Native handle for tests and future native helpers. */
  getNativeGraph(): JsMixGraph {
    return this.native
  }

  addInput(participantId: string): void {
    this.native.addInput(participantId)
  }

  removeInput(participantId: string): void {
    this.native.removeInput(participantId)
  }

  setMixingEnabled(enabled: boolean): void {
    this.native.setMixingEnabled(enabled)
  }

  mixingEnabled(): boolean {
    return this.native.mixingEnabled()
  }

  setGlobalMute(target: string, muted: boolean): void {
    this.native.setGlobalMute(target, muted)
  }

  isGloballyMuted(target: string): boolean {
    return this.native.isGloballyMuted(target)
  }

  setListenerMute(listener: string, target: string, muted: boolean): void {
    this.native.setListenerMute(listener, target, muted)
  }

  isListenerMuted(listener: string, target: string): boolean {
    return this.native.isListenerMuted(listener, target)
  }

  setListenerSources(listener: string, sources: string[]): void {
    this.native.setListenerSources(listener, sources)
  }

  listenerSources(listener: string): string[] | null {
    return this.native.listenerSources(listener) ?? null
  }

  clearListenerRoutes(listener: string): void {
    this.native.clearListenerRoutes(listener)
  }

  setPose(participantId: string, pose: ClientPose): void {
    this.native.setPose(participantId, toJsPose(pose))
  }

  clearPose(participantId: string): void {
    this.native.clearPose(participantId)
  }

  pose(participantId: string): ClientPose | null {
    const pose = this.native.pose(participantId)
    return pose ? fromJsPose(pose) : null
  }

  setPositionalEnabled(enabled: boolean): void {
    this.native.setPositionalEnabled(enabled)
  }

  positionalEnabled(): boolean {
    return this.native.positionalEnabled()
  }

  setDefaultMixPlacement(placement: MixPlacement): void {
    this.native.setDefaultMixPlacement(toJsPlacement(placement))
  }

  defaultMixPlacement(): MixPlacement {
    return fromJsPlacement(this.native.defaultMixPlacement())
  }

  setTtsMixPlacement(placement: MixPlacement): void {
    this.native.setTtsMixPlacement(toJsPlacement(placement))
  }

  ttsMixPlacement(): MixPlacement {
    return fromJsPlacement(this.native.ttsMixPlacement())
  }

  setDistanceParams(params: DistanceParams): void {
    this.native.setDistanceParams(toJsDistanceParams(params))
  }

  distanceParams(): DistanceParams {
    return fromJsDistanceParams(this.native.distanceParams())
  }

  setGroupMembers(groupId: string, members: string[]): void {
    this.native.setGroupMembers(groupId, members)
  }

  moveToGroup(participantId: string, groupId: string): void {
    this.native.moveToGroup(participantId, groupId)
  }

  removeFromGroup(participantId: string): void {
    this.native.removeFromGroup(participantId)
  }
}

/** Origin position for pose setup. */
export function vec3Zero(): Vec3 {
  return jsVec3Zero()
}

/** Identity quaternion for pose setup. */
export function quatIdentity(): Quat {
  return jsQuatIdentity()
}

export type { ClientPose, DistanceParams, MixPlacement, Quat, Vec3 } from './types.js'
export { MIX_PLACEMENT } from './types.js'
