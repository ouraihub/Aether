import { badRequest, KitFailure } from '@ouraihub/aether-core'

export interface CredentialScope {
  readonly applicationId: string
  readonly tenantId: string | null
  readonly subjectId: string | null
  readonly purpose: 'provider' | 'search' | 'mcp' | 'oauth'
  readonly recordId: string
}
export interface SealedCredential {
  readonly version: 1
  readonly keyId: string
  readonly nonce: string
  readonly ciphertext: string
}
export interface CredentialPort {
  seal(scope: CredentialScope, value: Uint8Array): Promise<SealedCredential>
  open(scope: CredentialScope, sealed: SealedCredential): Promise<Uint8Array>
  rotate(scope: CredentialScope, sealed: SealedCredential): Promise<SealedCredential>
}
const MAX_BYTES = 64 * 1024
function unreadable(): KitFailure {
  return new KitFailure('needs_config', 'Credential is unreadable', { reason: 'key_unreadable' })
}
function base64(bytes: Uint8Array): string {
  let text = ''
  for (const byte of bytes) text += String.fromCharCode(byte)
  return btoa(text)
}
function unbase64(text: string): Uint8Array<ArrayBuffer> {
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(text)) throw unreadable()
  const bytes = Uint8Array.from(atob(text), (character) => character.charCodeAt(0))
  if (base64(bytes) !== text) throw unreadable()
  return bytes
}
export function isSealedCredential(value: unknown): value is SealedCredential {
  return (
    typeof value === 'object' &&
    value !== null &&
    'version' in value &&
    value.version === 1 &&
    'keyId' in value &&
    typeof value.keyId === 'string' &&
    value.keyId.length > 0 &&
    value.keyId.length <= 128 &&
    'nonce' in value &&
    typeof value.nonce === 'string' &&
    value.nonce.length === 16 &&
    'ciphertext' in value &&
    typeof value.ciphertext === 'string' &&
    value.ciphertext.length >= 24 &&
    value.ciphertext.length <= Math.ceil((MAX_BYTES + 16) / 3) * 4
  )
}
function aad(scope: CredentialScope, keyId: string): Uint8Array<ArrayBuffer> {
  if (
    !scope.applicationId ||
    !scope.recordId ||
    scope.applicationId.length > 256 ||
    scope.recordId.length > 256 ||
    (scope.tenantId !== null && (!scope.tenantId || scope.tenantId.length > 256)) ||
    (scope.subjectId !== null && (!scope.subjectId || scope.subjectId.length > 256)) ||
    !['provider', 'search', 'mcp', 'oauth'].includes(scope.purpose)
  )
    throw badRequest('credentialScope', 'invalid')
  return new TextEncoder().encode(
    JSON.stringify([1, keyId, scope.applicationId, scope.tenantId, scope.subjectId, scope.purpose, scope.recordId]),
  )
}
/** Supply non-extractable AES-GCM keys from deployment secrets/KMS; old keys are read-only until explicit rotation. */
export function createCredentialPort(options: {
  readonly currentKeyId: string
  readonly keys: ReadonlyMap<string, CryptoKey>
}): CredentialPort {
  const keys = new Map(options.keys)
  const currentKeyId = options.currentKeyId
  if (!currentKeyId || currentKeyId.length > 128 || !keys.has(currentKeyId))
    throw new KitFailure('needs_config', 'Credential key configuration is required')
  for (const [id, key] of keys) {
    if (
      !id ||
      id.length > 128 ||
      key.type !== 'secret' ||
      key.algorithm.name !== 'AES-GCM' ||
      key.extractable ||
      !key.usages.includes('decrypt') ||
      (id === currentKeyId && !key.usages.includes('encrypt'))
    )
      throw new KitFailure('needs_config', 'Invalid credential key configuration')
  }
  async function seal(scope: CredentialScope, value: Uint8Array): Promise<SealedCredential> {
    if (value.byteLength > MAX_BYTES) throw badRequest('credential', 'too_large', MAX_BYTES)
    const key = keys.get(currentKeyId)
    if (!key) throw unreadable()
    const additionalData = aad(scope, currentKeyId)
    const nonce = crypto.getRandomValues(new Uint8Array(12))
    try {
      const ciphertext = await crypto.subtle.encrypt(
        { name: 'AES-GCM', iv: nonce, additionalData, tagLength: 128 },
        key,
        new Uint8Array(value),
      )
      return { version: 1, keyId: currentKeyId, nonce: base64(nonce), ciphertext: base64(new Uint8Array(ciphertext)) }
    } catch {
      throw unreadable()
    }
  }
  async function open(scope: CredentialScope, sealed: SealedCredential): Promise<Uint8Array> {
    try {
      if (!isSealedCredential(sealed)) throw unreadable()
      const key = keys.get(sealed.keyId)
      if (!key) throw unreadable()
      const nonce = unbase64(sealed.nonce)
      const ciphertext = unbase64(sealed.ciphertext)
      if (nonce.byteLength !== 12 || ciphertext.byteLength < 16 || ciphertext.byteLength > MAX_BYTES + 16)
        throw unreadable()
      return new Uint8Array(
        await crypto.subtle.decrypt(
          { name: 'AES-GCM', iv: nonce, additionalData: aad(scope, sealed.keyId), tagLength: 128 },
          key,
          ciphertext,
        ),
      )
    } catch {
      throw unreadable()
    }
  }
  return Object.freeze({
    seal,
    open,
    async rotate(scope: CredentialScope, sealed: SealedCredential) {
      const plaintext = await open(scope, sealed)
      try {
        return await seal(scope, plaintext)
      } finally {
        plaintext.fill(0)
      }
    },
  })
}
export async function importCredentialKey(encoded: string): Promise<CryptoKey> {
  try {
    const raw = unbase64(encoded)
    if (raw.byteLength !== 32) throw unreadable()
    try {
      return await crypto.subtle.importKey('raw', raw, 'AES-GCM', false, ['encrypt', 'decrypt'])
    } finally {
      raw.fill(0)
    }
  } catch {
    throw new KitFailure('needs_config', 'A 256-bit credential secret is required')
  }
}
