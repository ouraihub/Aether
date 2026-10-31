import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createCredentialPort,
  importCredentialKey,
  isSealedCredential,
  type CredentialScope,
} from './credentials.ts'

const scope: CredentialScope = {
  applicationId: 'app',
  tenantId: null,
  subjectId: 'alice',
  purpose: 'provider',
  recordId: 'provider/key',
}
test('AEAD binds every scope field, key ID, version and nonce; rotation reads old keys', async () => {
  const oldKey = await crypto.subtle.generateKey(
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
  const newKey = await crypto.subtle.generateKey(
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
  const old = createCredentialPort({
    currentKeyId: 'old',
    keys: new Map([['old', oldKey]]),
  })
  const bytes = new TextEncoder().encode('test-provider-credential')
  const sealed = await old.seal(scope, bytes)
  assert.equal(isSealedCredential(sealed), true)
  assert.notEqual(sealed.nonce, (await old.seal(scope, bytes)).nonce)
  assert.deepEqual(await old.open(scope, sealed), bytes)
  for (const changed of [
    { ...scope, applicationId: 'other' },
    { ...scope, tenantId: 'tenant' },
    { ...scope, subjectId: 'bob' },
    { ...scope, purpose: 'search' },
    { ...scope, recordId: 'other' },
  ]) {
    if (changed.purpose !== 'provider' && changed.purpose !== 'search')
      throw new Error('Unexpected test scope')
    await assert.rejects(
      old.open({ ...changed, purpose: changed.purpose }, sealed),
      { code: 'needs_config' },
    )
  }
  await assert.rejects(
    old.open(scope, { ...sealed, nonce: 'AAAAAAAAAAAAAAAA' }),
    { code: 'needs_config' },
  )
  await assert.rejects(old.open(scope, { ...sealed, keyId: 'new' }), {
    code: 'needs_config',
  })
  await assert.rejects(
    old.open(scope, {
      ...sealed,
      ciphertext: `${sealed.ciphertext[0] === 'A' ? 'B' : 'A'}${sealed.ciphertext.slice(1)}`,
    }),
    { code: 'needs_config' },
  )
  assert.equal(isSealedCredential({ ...sealed, version: 2 }), false)
  const rotating = createCredentialPort({
    currentKeyId: 'new',
    keys: new Map([
      ['old', oldKey],
      ['new', newKey],
    ]),
  })
  const rotated = await rotating.rotate(scope, sealed)
  assert.equal(rotated.keyId, 'new')
  assert.deepEqual(await rotating.open(scope, rotated), bytes)
  const afterRotation = createCredentialPort({
    currentKeyId: 'new',
    keys: new Map([['new', newKey]]),
  })
  await assert.rejects(afterRotation.open(scope, sealed), {
    code: 'needs_config',
  })
  assert.deepEqual(await afterRotation.open(scope, rotated), bytes)
})
test('credential configuration has no implicit default key', async () => {
  assert.throws(
    () => createCredentialPort({ currentKeyId: 'missing', keys: new Map() }),
    { code: 'needs_config' },
  )
  await assert.rejects(importCredentialKey('not base64'), {
    code: 'needs_config',
  })
  const key = await importCredentialKey(
    btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(32)))),
  )
  assert.equal(key.extractable, false)
})
