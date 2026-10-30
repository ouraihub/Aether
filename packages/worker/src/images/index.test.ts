import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { ImagePart } from '@ouraihub/aether-contracts'
import type { ImageConnection, ImageRequest } from '@ouraihub/aether-core/images'
import type { WorkerContext } from '../context.js'
import { ImageService } from './index.js'
import { emptyUsage } from '@ouraihub/aether-core/pricing'

const image: ImagePart = {
  mime: 'image/png',
  data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==',
}
const connection: ImageConnection = {
  modelId: 'image-model',
  baseUrl: 'https://images.example/v1',
  keys: [],
  headers: {},
  canVision: true,
}
const input: ImageRequest = { prompt: 'A tree', size: '1024x1024', images: [] }
const context: WorkerContext = {
  actor: {
    subjectId: 'user-1',
    kind: 'user',
    applicationId: 'app-1',
    tenantId: 'tenant-1',
    sessionId: 'session-1',
    scopes: [],
  },
  signal: new AbortController().signal,
  requestId: 'request-1',
  now: 1,
}

test('without persistence returns a display reference, never claiming it was saved', async () => {
  const service = new ImageService({
    outbound: {
      async fetch() {
        return Response.json({ data: [{ b64_json: image.data }] })
      },
    },
  })
  const result = await service.generate(connection, input, context)
  assert.equal(result.reference.url, `data:${image.mime};base64,${image.data}`)
  assert.equal(result.persisted, false)
})

test('persistence receives complete actor-bound Worker context', async () => {
  let saved = 0
  const service = new ImageService({
    outbound: {
      async fetch() {
        return Response.json({ data: [{ b64_json: image.data }] })
      },
    },
    async persist(value, current) {
      saved++
      assert.deepEqual(value, image)
      assert.equal(current, context)
      return { mime: value.mime, url: '/api/v1/files/generated-1' }
    },
  })
  const result = await service.generate(connection, input, context)
  assert.equal(result.reference.url, '/api/v1/files/generated-1')
  assert.equal(result.persisted, true)
  assert.equal(saved, 1)
})

test('save failure preserves generated fact and never repeats a paid call', async () => {
  let generated = 0
  const service = new ImageService({
    outbound: {
      async fetch() {
        generated++
        return Response.json({ data: [{ b64_json: image.data }] })
      },
    },
    async persist() {
      throw new Error('private storage failure')
    },
  })
  await assert.rejects(service.generate(connection, input, context), {
    dispatch: 'generated',
    code: 'internal',
    message: 'Image generated but persistence could not be confirmed',
    details: { field: 'images', reason: 'generated_save_failed' },
  })
  assert.equal(generated, 1)
})

test('generation failure never calls persistence', async () => {
  let saved = 0
  const service = new ImageService({
    outbound: {
      async fetch() {
        throw new Error('network')
      },
    },
    async persist(value) {
      saved++
      return { mime: value.mime, url: '/api/v1/files/unreachable' }
    },
  })
  await assert.rejects(service.generate(connection, input, context), { dispatch: 'unknown', code: 'network' })
  assert.equal(saved, 0)
})

test('service preserves provider usage and revised prompt instead of inventing chat totals', async () => {
  const service = new ImageService({
    outbound: {
      async fetch() {
        return Response.json({
          data: [{ b64_json: image.data, revised_prompt: 'One green tree' }],
          usage: { input_tokens: 13, output_tokens: 21 },
        })
      },
    },
  })
  const result = await service.generate(connection, input, context)
  assert.equal(result.revisedPrompt, 'One green tree')
  assert.deepEqual(result.usage, { ...emptyUsage(), inputTokens: 13, outputTokens: 21 })
})
