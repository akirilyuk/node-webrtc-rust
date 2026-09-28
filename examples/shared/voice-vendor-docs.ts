/**
 * Official API documentation links for VoiceAgent STT/TTS providers.
 *
 * Vendor ids, URLs, defaults, and allowlists come from `@node-webrtc-rust/voice-catalog`.
 * Sherpa local model list: `sherpa-local-model-catalog.json` (shared with download scripts).
 */

import {
  VOICE_VENDORS,
  getVendor,
  type VoiceVendorEntry,
  type VoiceVendorId,
} from '@node-webrtc-rust/voice-catalog'

import sherpaCatalog from './sherpa-local-model-catalog.json'

export type { VoiceVendorId } from '@node-webrtc-rust/voice-catalog'

export interface VendorDocLinks {
  /** SDK `stt.provider` / `tts.provider` id */
  id: VoiceVendorId
  label: string
  stt: boolean
  tts: boolean
  /** Default model id(s) used in examples */
  defaultModels?: { stt?: string; tts?: string }
  /** Official product or project home */
  home: string
  /** STT API reference (when stt is true) */
  sttDocs?: string
  /** TTS API reference (when tts is true) */
  ttsDocs?: string
  /** Model zoo / voice catalog when separate from API docs */
  modelsDocs?: string
}

export type SherpaLocalModelKind = 'transducer' | 'unavailable'

/** One row in `sherpa-local-model-catalog.json` — streaming Zipformer bundles for `local-sherpa`. */
export interface SherpaLocalModelEntry {
  id: string
  label: string
  bundle?: string
  language?: string
  kind: SherpaLocalModelKind
  note?: string
  approxMb?: string
}

export const SHERPA_ASR_RELEASE_BASE = sherpaCatalog.releaseBase
export const SHERPA_DEFAULT_MODEL_ID = sherpaCatalog.defaultModelId
export const SHERPA_LOCAL_MODEL_CATALOG = sherpaCatalog.models as SherpaLocalModelEntry[]
export const SHERPA_EXAMPLE_WORKSPACE = sherpaCatalog.exampleWorkspace

/** Default English STT bundle (`download-stt` / `download-stt:en`). */
export const SHERPA_DEFAULT_EN_STT_BUNDLE =
  SHERPA_LOCAL_MODEL_CATALOG.find((entry) => entry.id === SHERPA_DEFAULT_MODEL_ID)?.bundle ??
  'sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06'

/** Default Piper TTS voice bundled with the local Sherpa example. */
export const SHERPA_DEFAULT_EN_TTS_BUNDLE = 'vits-piper-en_US-amy-low'

const SHERPA_EXAMPLE_DIR = 'examples/voice-agent-local-sherpa'

/** Shell-ready default `SHERPA_STT_MODEL_PATH` after `download-stt:en`. */
export function defaultSherpaSttModelPath(repoRoot = '$PWD'): string {
  return `${repoRoot}/${SHERPA_EXAMPLE_DIR}/.models/${SHERPA_DEFAULT_EN_STT_BUNDLE}`
}

/** Shell-ready default `SHERPA_TTS_MODEL_PATH` after `download-tts:en`. */
export function defaultSherpaTtsModelPath(repoRoot = '$PWD'): string {
  return `${repoRoot}/${SHERPA_EXAMPLE_DIR}/.models/${SHERPA_DEFAULT_EN_TTS_BUNDLE}`
}

function entryToDocLinks(entry: VoiceVendorEntry): VendorDocLinks {
  const defaultModels =
    entry.defaultSttModel || entry.defaultTtsModel
      ? {
          ...(entry.defaultSttModel ? { stt: entry.defaultSttModel } : {}),
          ...(entry.defaultTtsModel ? { tts: entry.defaultTtsModel } : {}),
        }
      : undefined

  return {
    id: entry.id,
    label: entry.label,
    stt: entry.stt,
    tts: entry.tts,
    defaultModels,
    home: entry.home,
    sttDocs: entry.sttDocs,
    ttsDocs: entry.ttsDocs,
    modelsDocs: entry.modelsDocs,
  }
}

/** All supported providers — cloud + local + mock (from published catalog). */
export const VOICE_VENDOR_DOCS: VendorDocLinks[] = VOICE_VENDORS.map(entryToDocLinks)

export function getVendorDocs(id: string): VendorDocLinks | undefined {
  const entry = getVendor(id)
  return entry ? entryToDocLinks(entry) : undefined
}

/** Markdown table rows for README copy-paste (STT column). */
export function formatSttDocsTable(): string {
  return VOICE_VENDOR_DOCS.filter((v) => v.stt && v.id !== 'mock')
    .map((v) => {
      const model = v.defaultModels?.stt ? `\`${v.defaultModels.stt}\`` : '—'
      const link = v.sttDocs ?? v.home
      return `| ${v.label} | \`${v.id}\` | ${model} | [API docs](${link}) |`
    })
    .join('\n')
}

/** Markdown table rows for README copy-paste (TTS column). */
export function formatTtsDocsTable(): string {
  return VOICE_VENDOR_DOCS.filter((v) => v.tts && v.id !== 'mock')
    .map((v) => {
      const model = v.defaultModels?.tts ? `\`${v.defaultModels.tts}\`` : '—'
      const link = v.ttsDocs ?? v.home
      return `| ${v.label} | \`${v.id}\` | ${model} | [API docs](${link}) |`
    })
    .join('\n')
}

function sherpaDownloadScript(entry: SherpaLocalModelEntry): string {
  if (entry.id === 'en') {
    return '`download-stt` or `download-stt:en`'
  }
  return `\`download-stt:${entry.id}\``
}

/** Markdown table: Sherpa local STT languages + npm download scripts (see voice-agent-local-sherpa). */
export function formatSherpaLocalModelsTable(): string {
  return SHERPA_LOCAL_MODEL_CATALOG.map((entry) => {
    const bundle =
      entry.kind === 'transducer' && entry.bundle
        ? `\`…${entry.bundle.replace(/^sherpa-onnx-streaming-zipformer-/, '')}\``
        : '*unavailable*'
    const script = sherpaDownloadScript(entry)
    return `| ${entry.label} | \`${entry.id}\` | ${script} | ${bundle} |`
  }).join('\n')
}

/** Short usage block for Sherpa env vars and download commands. */
export function formatSherpaLocalModelsUsage(): string {
  const ws = SHERPA_EXAMPLE_WORKSPACE
  return `List all languages (including unavailable):

\`\`\`bash
npm run download-stt:list --workspace=${ws}
\`\`\`

Per-language download (examples):

\`\`\`bash
npm run download-stt:es --workspace=${ws}
npm run download-stt:de --workspace=${ws}
npm run download-stt --workspace=${ws} -- --lang=zh
\`\`\`

After download, export path and language (printed by the script):

\`\`\`bash
export SHERPA_STT_MODEL_PATH="${defaultSherpaSttModelPath()}"
export SHERPA_TTS_MODEL_PATH="${defaultSherpaTtsModelPath()}"
export SHERPA_STT_LANGUAGE=en   # optional — inferred from path when omitted
\`\`\`

For the **multilingual** Japanese/Arabic bundle (\`…-ar_en_id_ja_ru_th_vi_zh-2025-02-10\`), set \`SHERPA_STT_LANGUAGE\` to the language you speak: \`ja\`, \`ar\`, \`ru\`, \`vi\`, \`id\`, \`th\`, \`zh\`, or \`en\`.`
}
