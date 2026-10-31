import assert from 'node:assert/strict'
import test from 'node:test'
import type { R2Bucket } from './cloudflare.ts'
import type { Actor } from './context.ts'
import { R2FileStore } from './files.ts'
import { createLogger, type LogRecord } from './logger.ts'
const actor: Actor = {
  applicationId: 'app',
  tenantId: null,
  subjectId: 'alice',
  kind: 'user',
  sessionId: null,
  scopes: [],
}
export function testBucket(): {
  readonly bucket: R2Bucket
  readonly objects: Map<
    string,
    { readonly data: Uint8Array; readonly mime: string }
  >
} {
  const objects = new Map<string, { data: Uint8Array; mime: string }>()
  const bucket: R2Bucket = {
    async put(key, value, options) {
      if (options?.onlyIf?.etagDoesNotMatch === '*' && objects.has(key))
        return null
      objects.set(key, {
        data: new Uint8Array(value),
        mime: options?.httpMetadata?.contentType ?? '',
      })
      return { key }
    },
    async get(key) {
      const stored = objects.get(key)
      if (!stored) return null
      return {
        size: stored.data.byteLength,
        httpMetadata: { contentType: stored.mime },
        async arrayBuffer() {
          return new Uint8Array(stored.data).buffer
        },
      }
    },
    async delete(key) {
      objects.delete(key)
    },
  }
  return { bucket, objects }
}
test('R2 files isolate app/tenant/subject, validate MIME/size, and never expose bucket URLs', async () => {
  const { bucket } = testBucket()
  const files = new R2FileStore(bucket, { maxBytes: 32 })
  const data = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 1])
  await files.put(actor, 'image', { mime: 'image/png', data })
  assert.deepEqual((await files.get(actor, 'image'))?.data, data)
  for (const other of [
    { ...actor, applicationId: 'other' },
    { ...actor, tenantId: 'tenant' },
    { ...actor, subjectId: 'bob' },
  ])
    assert.equal(await files.get(other, 'image'), null)
  assert.equal(files.url(actor, 'image'), '/api/v1/files/image')
  await assert.rejects(files.put(actor, 'image', { mime: 'image/png', data }), {
    code: 'conflict',
  })
  await assert.rejects(
    files.put(actor, 'bad', { mime: 'image/png', data: new Uint8Array([1]) }),
    {
      code: 'bad_request',
    },
  )
  await assert.rejects(
    files.put(actor, 'large', {
      mime: 'application/octet-stream',
      data: new Uint8Array(33),
    }),
    {
      code: 'bad_request',
    },
  )
  await assert.rejects(
    files.put(actor, '../traversal', {
      mime: 'application/octet-stream',
      data: new Uint8Array([1]),
    }),
    { code: 'bad_request' },
  )
  await files.remove(actor, 'image')
  assert.equal(await files.get(actor, 'image'), null)
})
test('allowlist logging drops arbitrary secrets, raw URL, header objects and prompt fields', () => {
  const records: LogRecord[] = []
  const logger = createLogger({
    routes: ['/api/v1/runs/:id'],
    sink(record) {
      records.push(record)
    },
  })
  logger.log('request.finished', {
    requestId: 'request-1',
    route: '/api/v1/runs/:id',
    status: 200,
    durationMs: 4,
    secret: 'NEVERLOG',
    prompt: 'NEVERLOG',
    Authorization: 'Bearer NEVERLOG',
    headers: { Cookie: 'NEVERLOG' },
    output: 'NEVERLOG',
    url: 'https://app/?token=NEVERLOG',
  })
  assert.equal(JSON.stringify(records).includes('NEVERLOG'), false)
  assert.deepEqual(records[0]?.fields, {
    requestId: 'request-1',
    route: '/api/v1/runs/:id',
    status: 200,
    durationMs: 4,
  })
})
