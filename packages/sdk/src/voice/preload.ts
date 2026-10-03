import { preloadLanguageId as nativePreloadLanguageId } from '@node-webrtc-rust/bindings'

import type { LanguageIdConfig } from './types'

/**
 * Load the spoken-language-ID model (Sherpa Whisper tiny) into the process-wide pool now.
 *
 * Call once at host boot (e.g. a runner starting before any session exists). The returned
 * promise resolves when the model is resident; every {@link VoiceAgent} created afterwards
 * with the same `languageId.modelPath` shares that single instance, so the first utterance
 * does not pay the ~100 ms to multi-second model load. Calling it again, or concurrently, for
 * the same directory is a no-op after the first load.
 *
 * Constructing a `VoiceAgent` with `languageId` also starts this load in the background; use
 * `preloadLanguageId` when the load must finish before the first session starts.
 *
 * @param config - Same shape as `VoiceAgentConfig.languageId`; only `modelPath` is used.
 * @throws When `modelPath` is missing or is not a Whisper encoder/decoder directory.
 */
export async function preloadLanguageId(
  config: Pick<LanguageIdConfig, 'modelPath'> & Partial<LanguageIdConfig>,
): Promise<void> {
  await nativePreloadLanguageId(config)
}
