// Env-file parsing and per-vendor variable resolution for the bench CLI.

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

/**
 * Parses whitespace-separated `NAME=value` tokens. The value is everything after the first `=`.
 * A bare `NAME` is an empty string. Tokens whose name is not an identifier (`FOO#`) are ignored.
 * @param {string} text
 * @returns {Map<string, string>}
 */
export function parseEnvFile(text) {
  const map = new Map()
  for (const token of text.split(/\s+/)) {
    if (!token) continue
    const eq = token.indexOf('=')
    const name = eq < 0 ? token : token.slice(0, eq)
    const value = eq < 0 ? '' : token.slice(eq + 1)
    if (!NAME.test(name)) continue
    map.set(name, value)
  }
  return map
}

/**
 * Resolves the variables of one vendor. Each var takes the library name when set and non-empty in
 * the first source that has it, else the secret name.
 * @param {{ vars: { name: string, secret?: string, optional?: boolean }[], anyOf?: { envId: string, vars: string[] }[] }} vendor
 * @param {((name: string) => string | undefined)[]} sources ordered lookups (process env, env file)
 * @returns {{ env: Record<string, string>, missing: string[], sets?: string[] }}
 */
export function resolveVendorEnv(vendor, sources) {
  const lookup = (name) => {
    for (const source of sources) {
      const value = source(name)
      if (value) return value
    }
    return undefined
  }
  const env = {}
  const missing = []
  for (const v of vendor.vars) {
    const value = lookup(v.name) ?? (v.secret ? lookup(v.secret) : undefined)
    if (value) env[v.name] = value
    else if (!v.optional) missing.push(v.name)
  }
  const result = { env, missing }
  if (vendor.anyOf) {
    const complete = vendor.anyOf.filter((set) => set.vars.every((name) => env[name]))
    result.sets = complete.map((set) => set.envId)
    if (complete.length === 0) {
      missing.push(...vendor.anyOf.map((set) => set.vars.join(' + ')).map((s) => `(${s})`))
    }
  }
  return result
}
