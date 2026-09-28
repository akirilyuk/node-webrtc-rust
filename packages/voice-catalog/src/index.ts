import vendorsCatalog from '../catalog/vendors.json'
import azureTtsVoicesCatalog from '../catalog/azure-tts-voices.json'

/** SDK `stt.provider` / `tts.provider` id (matches VoiceAgent vendor ids). */
export type VoiceVendorId =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'cartesia'
  | 'assemblyai'
  | 'google'
  | 'groq'
  | 'azure'
  | 'aws'
  | 'local-sherpa'
  | 'mock'

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
  /** Relative to `catalog/` — load via {@link azureTtsVoices}. */
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

export interface VendorsCatalogFile {
  vendors: VoiceVendorEntry[]
}

export const vendorsCatalogJson: VendorsCatalogFile = vendorsCatalog as VendorsCatalogFile
export const azureTtsVoicesJson: AzureTtsVoicesCatalog =
  azureTtsVoicesCatalog as AzureTtsVoicesCatalog

/** All vendors from `catalog/vendors.json`. */
export const VOICE_VENDORS: readonly VoiceVendorEntry[] = vendorsCatalogJson.vendors

/** Azure TTS ShortNames snapshot (`catalog/azure-tts-voices.json`). */
export const azureTtsVoices: readonly string[] = azureTtsVoicesJson.voices

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

export function ttsVoiceAllowlist(vendorId: string): readonly string[] {
  if (vendorId === 'azure') {
    return azureTtsVoices
  }
  const vendor = getVendor(vendorId)
  return vendor?.ttsVoices ?? []
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
      allowlist = ttsVoiceAllowlist(vendorId)
      break
  }

  if (allowlist.length === 0) {
    // local-sherpa / google TTS voices / deepgram TTS models: no hardcoded zoo in catalog
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

export { vendorsCatalogJson as loadVendorsCatalog, azureTtsVoicesJson as loadAzureTtsVoicesCatalog }
