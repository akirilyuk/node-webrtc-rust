#!/usr/bin/env node
// Snapshots the Google Cloud Text-to-Speech voice names into catalog/google-tts-voices.json.
//
// Source: the static "List of all supported languages" table (no JS needed). We read the
// 4th column ("Voice name") of every body row of the first <table> after
// <h2 id="list_of_all_supported_languages">.
//
// Names without a "-" are EXCLUDED: those are the bare Gemini-TTS names (for example `Kore`).
// They need a `model_name` on the request, which nwr's REST and streaming calls do not send,
// so they cannot be used through nwr.
//
// Usage: npm run snapshot:google-tts-voices --workspace=@node-webrtc-rust/voice-catalog
/* global fetch -- Node 18+ global; the shared ESLint config does not list it for .mjs */
import { writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const SOURCE = 'https://docs.cloud.google.com/text-to-speech/docs/list-voices-and-types'
const VOICES_LIST_DOC =
  'https://docs.cloud.google.com/text-to-speech/docs/reference/rpc/google.cloud.texttospeech.v1'
const HEADING = '<h2 id="list_of_all_supported_languages"'
const EXPECTED = 2002
const TOLERANCE = 50
const VOICE_NAME_COLUMN = 3

const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' }

function decodeEntities(text) {
  return text.replace(/&(?:#(\d+)|#x([0-9a-fA-F]+)|([a-zA-Z]+));/g, (match, dec, hex, name) => {
    if (dec) return String.fromCodePoint(Number(dec))
    if (hex) return String.fromCodePoint(parseInt(hex, 16))
    return ENTITIES[name] ?? match
  })
}

function plainText(html) {
  return decodeEntities(html.replace(/<[^>]*>/g, '')).trim()
}

function fail(message) {
  console.error(`snapshot-google-tts-voices: ${message}`)
  process.exit(1)
}

const response = await fetch(SOURCE)
if (!response.ok) fail(`GET ${SOURCE} -> HTTP ${response.status}`)
const html = await response.text()

const headingAt = html.indexOf(HEADING)
if (headingAt < 0) fail(`heading ${HEADING} not found`)
const tableStart = html.indexOf('<table', headingAt)
if (tableStart < 0) fail('no <table> after the heading')
const tableEnd = html.indexOf('</table>', tableStart)
if (tableEnd < 0) fail('unterminated <table>')
const table = html.slice(tableStart, tableEnd)

const seen = new Set()
const voices = []
let rows = 0
for (const rowMatch of table.matchAll(/<tr\b[^>]*>([\s\S]*?)<\/tr>/g)) {
  const cells = [...rowMatch[1].matchAll(/<td\b[^>]*>([\s\S]*?)<\/td>/g)].map((m) => m[1])
  if (cells.length === 0) continue // header row
  rows += 1
  if (cells.length <= VOICE_NAME_COLUMN) fail(`row ${rows} has only ${cells.length} cells`)
  const name = plainText(cells[VOICE_NAME_COLUMN])
  if (!name.includes('-')) continue
  if (seen.has(name)) continue
  seen.add(name)
  voices.push(name)
}

if (voices.length < EXPECTED - TOLERANCE || voices.length > EXPECTED + TOLERANCE) {
  fail(
    `voice count ${voices.length} is outside ${EXPECTED - TOLERANCE}..${EXPECTED + TOLERANCE} ` +
      `(${rows} body rows); the page layout probably changed`,
  )
}

const snapshot = {
  snapshotDate: new Date().toISOString().slice(0, 10),
  source: SOURCE,
  voicesListDoc: VOICES_LIST_DOC,
  voices,
}

const out = join(dirname(fileURLToPath(import.meta.url)), '..', 'catalog', 'google-tts-voices.json')
writeFileSync(out, `${JSON.stringify(snapshot, null, 2)}\n`)
console.log(`wrote ${voices.length} voices (${rows} body rows) to ${out}`)
