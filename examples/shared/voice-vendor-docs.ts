/**
 * Official API documentation links for VoiceAgent STT/TTS providers.
 *
 * Single source of truth — referenced from example READMEs and presets.
 * Update here when adding a vendor or changing default models.
 *
 * Sherpa local model list: `sherpa-local-model-catalog.json` (shared with download scripts).
 */

import sherpaCatalog from './sherpa-local-model-catalog.json'

export type VoiceVendorId =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'cartesia'
  | 'assemblyai'
  | 'google'
  | 'local-sherpa'
  | 'mock'

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

/** All supported providers — cloud + local + mock. */
export const VOICE_VENDOR_DOCS: VendorDocLinks[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    stt: true,
    tts: true,
    defaultModels: { stt: 'whisper-1', tts: 'tts-1' },
    home: 'https://platform.openai.com/docs',
    sttDocs: 'https://platform.openai.com/docs/guides/speech-to-text',
    ttsDocs: 'https://platform.openai.com/docs/guides/text-to-speech',
    modelsDocs: 'https://platform.openai.com/docs/models',
  },
  {
    id: 'deepgram',
    label: 'Deepgram',
    stt: true,
    tts: false,
    defaultModels: { stt: 'nova-2' },
    home: 'https://developers.deepgram.com/',
    sttDocs: 'https://developers.deepgram.com/docs/live-streaming-audio',
    modelsDocs: 'https://developers.deepgram.com/docs/models',
  },
  {
    id: 'elevenlabs',
    label: 'ElevenLabs',
    stt: false,
    tts: true,
    defaultModels: { tts: 'eleven_multilingual_v2' },
    home: 'https://elevenlabs.io/docs',
    ttsDocs: 'https://elevenlabs.io/docs/api-reference/text-to-speech/convert',
    modelsDocs: 'https://elevenlabs.io/docs/voices',
  },
  {
    id: 'cartesia',
    label: 'Cartesia',
    stt: false,
    tts: true,
    defaultModels: { tts: 'sonic-english' },
    home: 'https://docs.cartesia.ai/',
    ttsDocs: 'https://docs.cartesia.ai/api-reference/tts/bytes',
    modelsDocs: 'https://docs.cartesia.ai/models',
  },
  {
    id: 'assemblyai',
    label: 'AssemblyAI',
    stt: true,
    tts: false,
    defaultModels: { stt: 'universal-streaming-english' },
    home: 'https://www.assemblyai.com/docs',
    sttDocs: 'https://www.assemblyai.com/docs/speech-to-text/streaming',
    modelsDocs: 'https://www.assemblyai.com/docs/speech-to-text/pre-recorded-audio/select-the-speech-model',
  },
  {
    id: 'google',
    label: 'Google Cloud',
    stt: true,
    tts: true,
    defaultModels: { stt: 'latest_long', tts: 'en-US-Neural2-A' },
    home: 'https://cloud.google.com/speech-to-text',
    sttDocs: 'https://cloud.google.com/speech-to-text/docs',
    ttsDocs: 'https://cloud.google.com/text-to-speech/docs',
    modelsDocs: 'https://cloud.google.com/text-to-speech/docs/voices',
  },
  {
    id: 'local-sherpa',
    label: 'Sherpa-ONNX (local)',
    stt: true,
    tts: false,
    defaultModels: { stt: 'sherpa-onnx-streaming-zipformer-en-2023-06-26' },
    home: 'https://github.com/k2-fsa/sherpa-onnx',
    sttDocs: 'https://k2-fsa.github.io/sherpa/onnx/',
    modelsDocs: 'https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models',
  },
  {
    id: 'mock',
    label: 'Mock (CI / local)',
    stt: true,
    tts: true,
    home: 'https://github.com/akirilyuk/node-webrtc-rust/tree/main/crates/vendor-mock',
  },
]

export function getVendorDocs(id: string): VendorDocLinks | undefined {
  return VOICE_VENDOR_DOCS.find((entry) => entry.id === id)
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
    return '`download-model` or `download-model:en`'
  }
  return `\`download-model:${entry.id}\``
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
npm run download-model:list --workspace=${ws}
\`\`\`

Per-language download (examples):

\`\`\`bash
npm run download-model:es --workspace=${ws}
npm run download-model:de --workspace=${ws}
npm run download-model --workspace=${ws} -- --lang=zh
\`\`\`

After download, export path and language (printed by the script):

\`\`\`bash
export SHERPA_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/<bundle-name>"
export SHERPA_LANGUAGE=de   # optional — inferred from path when omitted
\`\`\`

For the **multilingual** Japanese/Arabic bundle (\`…-ar_en_id_ja_ru_th_vi_zh-2025-02-10\`), set \`SHERPA_LANGUAGE\` to the language you speak: \`ja\`, \`ar\`, \`ru\`, \`vi\`, \`id\`, \`th\`, \`zh\`, or \`en\`.`
}
