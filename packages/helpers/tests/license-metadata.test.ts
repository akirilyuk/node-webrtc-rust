import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const root = resolve(__dirname, '../../..')
const rootLicense = readFileSync(join(root, 'LICENSE'), 'utf8')

const packageDirs = [
  ...readdirSync(join(root, 'packages'), { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => join(root, 'packages', d.name))
    .filter((dir) => existsSync(join(dir, 'package.json'))),
  ...readdirSync(join(root, 'packages/bindings/npm'), { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => join(root, 'packages/bindings/npm', d.name)),
]

describe('license metadata', () => {
  it('root LICENSE is Business Source License 1.1', () => {
    expect(rootLicense).toContain('Business Source License 1.1')
    expect(rootLicense).toContain('Change License:       Apache License, Version 2.0')
  })

  it('root package.json and Cargo.toml use BUSL-1.1', () => {
    const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))
    expect(pkg.license).toBe('BUSL-1.1')
    expect(readFileSync(join(root, 'Cargo.toml'), 'utf8')).toMatch(/^license = "BUSL-1\.1"$/m)
  })

  it('finds the published packages', () => {
    expect(packageDirs.length).toBeGreaterThanOrEqual(11)
  })

  for (const dir of packageDirs) {
    it(`${dir.slice(root.length + 1)} has BUSL-1.1, LICENSE in files and a matching LICENSE file`, () => {
      const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
      expect(pkg.license).toBe('BUSL-1.1')
      expect(pkg.files).toContain('LICENSE')
      expect(readFileSync(join(dir, 'LICENSE'), 'utf8')).toBe(rootLicense)
    })
  }
})
