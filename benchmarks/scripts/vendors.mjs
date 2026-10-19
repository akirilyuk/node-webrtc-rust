// The cloud speech vendors measured by `crates/vendor-<id>/tests/live_bench.rs`.
//
//   id        crate suffix (`crates/vendor-<id>`)
//   envId     first argument the bench passes to `live_env`; `<ENVID_UPPER>_LIVE_REQUIRED=1` makes
//             a missing variable a failure instead of a skip
//   vars      environment variables the bench reads. `name` is the library name; `secret` is the
//             CI secret name mapped to it in `.github/workflows/vendor-bench.yml`, accepted as an
//             alias in an env file
//   unpublished  reason string; vendors with `unpublished` are skipped by `--vendors all` with this
//             reason; naming them in `--vendors` runs them
//   hosts     API hosts the crate's default configuration connects to for the transports the bench
//             uses. `{region}` is filled from `regionVar`; the probe skips a templated host when
//             that variable is unset. `vendors.test.mjs` checks every host against the crate source.

/** @typedef {{ name: string, secret?: string, optional?: boolean }} VendorVar */

export const VENDORS = [
  {
    id: 'deepgram',
    unpublished: 'vendor terms do not allow benchmarking',
    envId: 'deepgram',
    vars: [{ name: 'DEEPGRAM_API_KEY', secret: 'DEEPGRAM_LIVE_KEY' }],
    hosts: ['api.deepgram.com'],
  },
  {
    id: 'elevenlabs',
    envId: 'elevenlabs',
    vars: [{ name: 'ELEVENLABS_API_KEY', secret: 'ELEVENLABS_LIVE_KEY' }],
    hosts: ['api.elevenlabs.io'],
  },
  {
    id: 'cartesia',
    envId: 'cartesia',
    vars: [
      { name: 'CARTESIA_API_KEY', secret: 'CARTESIA_LIVE_KEY' },
      { name: 'CARTESIA_VOICE_ID', secret: 'CARTESIA_LIVE_VOICE_ID' },
    ],
    hosts: ['api.cartesia.ai'],
  },
  {
    id: 'assemblyai',
    unpublished: 'vendor terms do not allow benchmarking',
    envId: 'assemblyai',
    vars: [{ name: 'ASSEMBLYAI_API_KEY', secret: 'ASSEMBLYAI_LIVE_KEY' }],
    // Default is the US host: crates/vendor-assemblyai/src/matrix.rs:16 (STREAMING_WS_BASE
    // `wss://streaming.assemblyai.com/v3/ws`). The EU host only applies with an explicit
    // `endpoint`, which the bench does not set.
    hosts: ['streaming.assemblyai.com'],
  },
  {
    id: 'google',
    unpublished: 'no account yet',
    envId: 'google',
    // Two credential paths, each gated by its own flag (live_bench.rs): API key (REST) or
    // service account (gRPC, `google_sa`). The vendor runs when at least one set is complete.
    vars: [
      { name: 'GOOGLE_API_KEY', secret: 'GOOGLE_LIVE_API_KEY', optional: true },
      { name: 'GOOGLE_APPLICATION_CREDENTIALS', optional: true },
      { name: 'GOOGLE_CLOUD_PROJECT', secret: 'GOOGLE_LIVE_PROJECT_ID', optional: true },
    ],
    anyOf: [
      { envId: 'google', vars: ['GOOGLE_API_KEY'] },
      { envId: 'google_sa', vars: ['GOOGLE_APPLICATION_CREDENTIALS', 'GOOGLE_CLOUD_PROJECT'] },
    ],
    hosts: ['texttospeech.googleapis.com', 'speech.googleapis.com'],
  },
  {
    id: 'groq',
    envId: 'groq',
    vars: [{ name: 'GROQ_API_KEY', secret: 'GROQ_LIVE_KEY' }],
    hosts: ['api.groq.com'],
  },
  {
    id: 'azure',
    unpublished: 'no account yet',
    envId: 'azure',
    vars: [
      { name: 'AZURE_SPEECH_KEY', secret: 'AZURE_LIVE_SPEECH_KEY' },
      { name: 'AZURE_SPEECH_REGION', secret: 'AZURE_LIVE_SPEECH_REGION' },
    ],
    regionVar: 'AZURE_SPEECH_REGION',
    hosts: ['{region}.tts.speech.microsoft.com', '{region}.api.cognitive.microsoft.com'],
  },
  {
    id: 'aws',
    unpublished: 'no account yet',
    envId: 'aws',
    vars: [
      { name: 'AWS_ACCESS_KEY_ID', secret: 'AWS_LIVE_ACCESS_KEY_ID' },
      { name: 'AWS_SECRET_ACCESS_KEY', secret: 'AWS_LIVE_SECRET_ACCESS_KEY' },
      { name: 'AWS_REGION', secret: 'AWS_LIVE_REGION' },
    ],
    regionVar: 'AWS_REGION',
    // The crate has no host literal: the AWS SDK resolves the regional endpoints of these two
    // services (`vendors.test.mjs` checks the crate depends on both SDK crates instead).
    sdkCrates: ['aws-sdk-polly', 'aws-sdk-transcribestreaming'],
    hosts: ['polly.{region}.amazonaws.com', 'transcribestreaming.{region}.amazonaws.com'],
  },
  {
    id: 'openai',
    envId: 'openai',
    vars: [{ name: 'OPENAI_API_KEY', secret: 'OPENAI_LIVE_KEY' }],
    hosts: ['api.openai.com'],
  },
  {
    id: 'fishaudio',
    envId: 'fish',
    vars: [{ name: 'FISH_API_KEY', secret: 'FISH_AUDIO_API' }],
    hosts: ['api.fish.audio'],
  },
]

/** Every variable and secret name of every vendor (removed from the child env of a bench). */
export function allVendorEnvNames() {
  const names = new Set()
  for (const vendor of VENDORS) {
    for (const v of vendor.vars) {
      names.add(v.name)
      if (v.secret) names.add(v.secret)
    }
  }
  return [...names]
}
