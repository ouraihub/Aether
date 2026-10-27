import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

export function checkGate(scope, results) {
  if (!['docs', 'automation', 'frontend', 'full'].includes(scope) || results.scope !== 'success') return false
  const required = scope === 'full' ? ['windows'] : ['light']
  const unused = scope === 'full' ? ['light'] : ['windows']
  return required.every((job) => results[job] === 'success') && unused.every((job) => results[job] === 'skipped')
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const needs = JSON.parse(process.env.CI_NEEDS || '{}')
  const results = Object.fromEntries(Object.entries(needs).map(([job, value]) => [job, value.result]))
  if (!checkGate(needs.scope?.outputs?.scope, results)) throw new Error('Required CI checks did not succeed')
  console.log('All checks required for this change succeeded')
}
