import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import * as validators from '../src/validators.js'

const root = new URL('../../../contracts/', import.meta.url)
const fixtures = JSON.parse(readFileSync(new URL('cases.json', root), 'utf8'))
for (const fixture of fixtures.cases) {
  test(`${fixture.id}: standalone ${fixture.contract}`, () => {
    const validate = validators[`validate${fixture.contract}`]
    assert.equal(validate(fixture.value), fixture.valid, JSON.stringify(validate.errors))
  })
}
test('standalone assets contain no dynamic evaluation or CommonJS runtime imports', () => {
  const source = readFileSync(fileURLToPath(new URL('../src/validators.js', import.meta.url)), 'utf8')
  assert.doesNotMatch(source, /\beval\s*\(|new\s+Function\s*\(|\brequire\s*\(/)
})
test('capability validators reject malformed nested secret-safe DTOs', () => {
  assert.equal(validators.validateProvider({ id: 'p', name: 'n', enabled: true, notes: '', presetId: null, endpoints: [] }), true)
  assert.equal(validators.validateProvider({ id: 'p', name: 'n', enabled: true, notes: '', presetId: null, endpoints: [{ apiKey: 'secret' }] }), false)
  assert.equal(validators.validateSearchPage({ title: 'Page', url: 'https://example.org/', content: 'Body' }), true)
  assert.equal(validators.validateSearchPage({ title: 'Page', url: 42, content: 'Body' }), false)
})
