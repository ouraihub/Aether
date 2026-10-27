import assert from 'node:assert/strict'
import test from 'node:test'
import { checkGate } from './ci-gate.mjs'

for (const scope of ['docs', 'automation', 'frontend', 'full']) {
  const results = {
    scope: 'success',
    light: scope === 'full' ? 'skipped' : 'success',
    windows: scope === 'full' ? 'success' : 'skipped',
  }
  test(`${scope} passes only when every selected job succeeds`, () => {
    assert.equal(checkGate(scope, results), true)
    for (const job of Object.keys(results).filter((key) => results[key] === 'success')) {
      for (const result of ['failure', 'cancelled', 'skipped', '', undefined]) {
        assert.equal(checkGate(scope, { ...results, [job]: result }), false, `${job}: ${result}`)
      }
    }
  })
}
test('missing or unknown classification cannot pass', () => {
  for (const scope of ['', undefined, 'native']) assert.equal(checkGate(scope, {}), false)
})

test('nightly Unix failures do not block the independent daily Windows gate', () => {
  assert.equal(
    checkGate('full', {
      scope: 'success',
      light: 'skipped',
      windows: 'success',
      'macos-core': 'failure',
      'linux-core': 'cancelled',
    }),
    true,
  )
})
