import vendorsCatalog from '../catalog/vendors.json'
import azureTtsVoicesCatalog from '../catalog/azure-tts-voices.json'
import googleTtsVoicesCatalog from '../catalog/google-tts-voices.json'

/** SDK `stt.provider` / `tts.provider` id (matches VoiceAgent vendor ids). */
export type VoiceVendorId =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'cartesia'
  | 'fishaudio'
  | 'assemblyai'
  | 'google'
  | 'groq'
  | 'azure'
  | 'aws'
  | 'local-sherpa'
  | 'mock'

/** Concrete speech transport a customer can select (`auto` is the SDK-level alias for the catalog default). */
export type VoiceTransport = 'websocket' | 'http'

export interface VoiceVendorEntry {
  id: VoiceVendorId
  label: string
  stt: boolean
  tts: boolean
  defaultSttModel?: string
  defaultTtsModel?: string
  defaultTtsVoice?: string
  sttModels: string[]
  ttsModels: string[]
  ttsVoices: string[]
  /** Transports the customer may select for STT. Omitted or empty when the model decides. */
  sttTransports?: VoiceTransport[]
  /** Transports the customer may select for TTS. Omitted or empty when the voice or model decides. */
  ttsTransports?: VoiceTransport[]
  /** Transport `auto` resolves to for STT (today's behaviour). Present only when `sttTransports` is non-empty. */
  defaultSttTransport?: VoiceTransport
  /** Transport `auto` resolves to for TTS (today's behaviour). Present only when `ttsTransports` is non-empty. */
  defaultTtsTransport?: VoiceTransport
  /** Per-model voice list; overrides `ttsVoices` for that model. */
  ttsVoicesByModel?: Record<string, string[]>
  /**
   * How voice ids are defined for this vendor.
   * - `listed`: `ttsVoices` / `ttsVoicesByModel` / `ttsVoiceListFile` is the complete documented list.
   * - `open`: voice ids are account or library ids plus customer clones; no closed list exists.
   * - `model`: the voice is selected by the model id.
   */
  ttsVoiceIds?: 'listed' | 'open' | 'model'
  /** Relative to `catalog/` — load via {@link azureTtsVoices} / {@link googleTtsVoices}. */
  ttsVoiceListFile?: string
  home: string
  sttDocs?: string
  ttsDocs?: string
  modelsDocs?: string
  snapshotDate?: string
}

export interface AzureTtsVoicesCatalog {
  snapshotDate: string
  source: string
  voicesListDoc: string
  voices: string[]
}

export interface GoogleTtsVoicesCatalog {
  snapshotDate: string
  source: string
  voicesListDoc: string
  voices: string[]
}

export interface VendorsCatalogFile {
  vendors: VoiceVendorEntry[]
}

export const vendorsCatalogJson: VendorsCatalogFile = vendorsCatalog as VendorsCatalogFile
export const azureTtsVoicesJson: AzureTtsVoicesCatalog =
  azureTtsVoicesCatalog as AzureTtsVoicesCatalog
export const googleTtsVoicesJson: GoogleTtsVoicesCatalog =
  googleTtsVoicesCatalog as GoogleTtsVoicesCatalog

/** All vendors from `catalog/vendors.json`. */
export const VOICE_VENDORS: readonly VoiceVendorEntry[] = vendorsCatalogJson.vendors

/** Azure TTS ShortNames snapshot (`catalog/azure-tts-voices.json`). */
export const azureTtsVoices: readonly string[] = azureTtsVoicesJson.voices

/** Google TTS voice names snapshot (`catalog/google-tts-voices.json`). */
export const googleTtsVoices: readonly string[] = googleTtsVoicesJson.voices

export function getVendor(id: string): VoiceVendorEntry | undefined {
  return VOICE_VENDORS.find((entry) => entry.id === id)
}

export function allVendorIds(): VoiceVendorId[] {
  return VOICE_VENDORS.map((entry) => entry.id)
}

/** STT-capable providers for customer configuration (excludes `mock`). */
export function customerSttProviderIds(): VoiceVendorId[] {
  return VOICE_VENDORS.filter((entry) => entry.stt && entry.id !== 'mock').map((entry) => entry.id)
}

/** TTS-capable providers for customer configuration (excludes `mock`). */
export function customerTtsProviderIds(): VoiceVendorId[] {
  return VOICE_VENDORS.filter((entry) => entry.tts && entry.id !== 'mock').map((entry) => entry.id)
}

export function sttModelAllowlist(vendorId: string): readonly string[] {
  const vendor = getVendor(vendorId)
  return vendor?.sttModels ?? []
}

export function ttsModelAllowlist(vendorId: string): readonly string[] {
  const vendor = getVendor(vendorId)
  return vendor?.ttsModels ?? []
}

/** Documented TTS voices for a vendor; `model` selects a per-model list when the vendor has one. */
export function ttsVoiceAllowlist(vendorId: string, model?: string): readonly string[] {
  const vendor = getVendor(vendorId)
  const byModel = model ? vendor?.ttsVoicesByModel?.[model] : undefined
  if (byModel) {
    return byModel
  }
  if (vendorId === 'azure') {
    return azureTtsVoices
  }
  if (vendorId === 'google') {
    return googleTtsVoices
  }
  return vendor?.ttsVoices ?? []
}

/** Selectable STT transports for a vendor (empty when none). */
export function sttTransports(vendorId: string): readonly VoiceTransport[] {
  return getVendor(vendorId)?.sttTransports ?? []
}

/** Selectable TTS transports for a vendor (empty when none). */
export function ttsTransports(vendorId: string): readonly VoiceTransport[] {
  return getVendor(vendorId)?.ttsTransports ?? []
}

/** Transport `auto` resolves to for STT, or `undefined` when the vendor has no selectable STT transport. */
export function defaultSttTransport(vendorId: string): VoiceTransport | undefined {
  return getVendor(vendorId)?.defaultSttTransport
}

/** Transport `auto` resolves to for TTS, or `undefined` when the vendor has no selectable TTS transport. */
export function defaultTtsTransport(vendorId: string): VoiceTransport | undefined {
  return getVendor(vendorId)?.defaultTtsTransport
}

export type VoiceCatalogValidationKind = 'sttModel' | 'ttsModel' | 'ttsVoice'

export interface VoiceCatalogValidationError {
  kind: VoiceCatalogValidationKind
  vendorId: string
  value: string
  message: string
}

/** Returns `null` when the model/voice is in the published allowlist for that vendor. */
export function validateVoiceCatalogChoice(
  vendorId: string,
  kind: VoiceCatalogValidationKind,
  value: string,
  model?: string,
): VoiceCatalogValidationError | null {
  const vendor = getVendor(vendorId)
  if (!vendor) {
    return {
      kind,
      vendorId,
      value,
      message: `unknown vendor id \`${vendorId}\``,
    }
  }

  let allowlist: readonly string[]
  switch (kind) {
    case 'sttModel':
      if (!vendor.stt) {
        return {
          kind,
          vendorId,
          value,
          message: `vendor \`${vendorId}\` does not support STT`,
        }
      }
      allowlist = sttModelAllowlist(vendorId)
      break
    case 'ttsModel':
      if (!vendor.tts) {
        return {
          kind,
          vendorId,
          value,
          message: `vendor \`${vendorId}\` does not support TTS`,
        }
      }
      allowlist = ttsModelAllowlist(vendorId)
      break
    case 'ttsVoice':
      if (!vendor.tts) {
        return {
          kind,
          vendorId,
          value,
          message: `vendor \`${vendorId}\` does not support TTS`,
        }
      }
      allowlist = ttsVoiceAllowlist(vendorId, model)
      break
  }

  if (allowlist.length === 0) {
    // An empty list means the vendor's `ttsVoiceIds` is `open` (account/library ids) or `model`
    // (voice chosen by the model id); there is no closed list to check against.
    return null
  }

  if (!allowlist.includes(value)) {
    return {
      kind,
      vendorId,
      value,
      message: `unsupported ${kind} \`${value}\` for vendor \`${vendorId}\``,
    }
  }

  return null
}

export {
  vendorsCatalogJson as loadVendorsCatalog,
  azureTtsVoicesJson as loadAzureTtsVoicesCatalog,
  googleTtsVoicesJson as loadGoogleTtsVoicesCatalog,
}
