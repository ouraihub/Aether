import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { OutboundHttp } from '../ports.js'
import { emptyUsage } from '../pricing.js'
import {
  checkImages,
  generateImage,
  IMAGE_TIMEOUT_MS,
  ImageFailure,
  MAX_IMAGE_BYTES,
  type ImageConnection,
  type ImageRequest,
  imageCapabilities,
  validateImageBytes,
} from './index.js'

const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg=='
const connection: ImageConnection = {
  baseUrl: 'https://images.example/v1',
  modelId: 'image-model',
  keys: ['first-secret', 'second-secret'],
  headers: { Authorization: 'custom-secret', 'Content-Type': 'application/json', 'x-custom': 'yes' },
  canVision: true,
}
const input: ImageRequest = { prompt: '  Draw a tree  ', size: '1024x1024', images: [] }
const signal = (): AbortSignal => new AbortController().signal
const success = (): Response => Response.json({ data: [{ b64_json: PNG }] })

test('generation uses explicit transport, trimmed prompt, headers, and no response_format', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch(url, init) {
      calls++
      assert.equal(url, 'https://images.example/v1/images/generations')
      assert.equal(init.redirect, 'manual')
      assert.equal(new Headers(init.headers).get('authorization'), 'Bearer first-secret')
      assert.equal(new Headers(init.headers).get('x-custom'), 'yes')
      assert.equal(init.body, JSON.stringify({ model: 'image-model', prompt: 'Draw a tree', size: '1024x1024', n: 1 }))
      return success()
    },
  }
  assert.deepEqual(await generateImage(http, connection, input, signal()), {
    image: { mime: 'image/png', data: PNG },
    revisedPrompt: null,
    usage: emptyUsage(),
    totalTokens: null,
  })
  assert.equal(calls, 1)
})

for (const count of [1, 2, 4])
  test(`editing ${count} references uses native multipart MIME and filenames`, async () => {
    const images = Array.from({ length: count }, () => ({ mime: 'image/png', data: PNG }))
    const http: OutboundHttp = {
      async fetch(url, init) {
        assert.equal(url, 'https://edits.example/images/edits')
        assert.equal(new Headers(init.headers).has('content-type'), false)
        assert.ok(init.body instanceof FormData)
        assert.equal(init.body.get('model'), 'image-model')
        assert.equal(init.body.get('prompt'), 'Draw a tree')
        assert.equal(init.body.get('size'), '1536x1024')
        assert.equal(init.body.get('n'), '1')
        const files = init.body.getAll(count === 1 ? 'image' : 'image[]')
        assert.equal(files.length, count)
        for (const [index, file] of files.entries()) {
          assert.notEqual(typeof file, 'string')
          if (typeof file === 'string') throw new Error('Expected file')
          assert.equal(file.name, `image-${index + 1}.png`)
          assert.equal(file.type, 'image/png')
          assert.deepEqual(
            new Uint8Array(await file.arrayBuffer()),
            Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0)),
          )
        }
        return success()
      },
    }
    await generateImage(
      http,
      { ...connection, imageEditBaseUrl: 'https://edits.example' },
      { ...input, size: '1536x1024', images },
      signal(),
    )
  })

for (const status of [401, 403, 429])
  test(`credential rejection ${status} advances key`, async () => {
    const keys: (string | null)[] = []
    const http: OutboundHttp = {
      async fetch(_url, init) {
        keys.push(new Headers(init.headers).get('authorization'))
        return keys.length === 1 ? new Response('secret upstream body', { status }) : success()
      },
    }
    await generateImage(http, connection, input, signal())
    assert.deepEqual(keys, ['Bearer first-secret', 'Bearer second-secret'])
  })

for (const status of [400, 404, 500, 503])
  test(`rejection ${status} never retries or exposes provider text`, async () => {
    let calls = 0
    const http: OutboundHttp = {
      async fetch() {
        calls++
        return new Response('first-secret leaked', { status })
      },
    }
    await assert.rejects(generateImage(http, connection, input, signal()), {
      dispatch: status >= 500 ? 'unknown' : 'rejected',
      code: 'upstream',
      details: { status },
      message: 'Image provider rejected request',
    })
    assert.equal(calls, 1)
  })

test('disconnect after dispatch is unknown and never retries', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch() {
      calls++
      throw new Error('first-secret')
    },
  }
  await assert.rejects(generateImage(http, connection, input, signal()), {
    dispatch: 'unknown',
    code: 'network',
    message: 'Image provider unreachable',
  })
  assert.equal(calls, 1)
})

test('validation and pre-cancellation never dispatch', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch() {
      calls++
      return success()
    },
  }
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(generateImage(http, connection, input, controller.signal), {
    code: 'cancelled',
    dispatch: 'not_dispatched',
  })
  await assert.rejects(generateImage(http, connection, { ...input, prompt: ' ' }, signal()), {
    code: 'bad_request',
    dispatch: 'not_dispatched',
  })
  await assert.rejects(
    generateImage(
      http,
      { ...connection, canVision: false },
      { ...input, images: [{ mime: 'image/png', data: PNG }] },
      signal(),
    ),
    { code: 'unsupported', dispatch: 'not_dispatched' },
  )
  await assert.rejects(
    generateImage(
      http,
      connection,
      { ...input, images: Array.from({ length: 5 }, () => ({ mime: 'image/png', data: PNG })) },
      signal(),
    ),
    { code: 'bad_request', dispatch: 'not_dispatched' },
  )
  assert.equal(calls, 0)
})

test('download uses same injected policy without forwarding provider credentials', async () => {
  const calls: string[] = []
  const http: OutboundHttp = {
    async fetch(url, init) {
      calls.push(url)
      if (calls.length === 1) return Response.json({ data: [{ url: 'https://cdn.example/image.png' }] })
      assert.equal(new Headers(init.headers).has('authorization'), false)
      assert.equal(init.redirect, 'manual')
      return new Response(
        Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0)),
        { headers: { 'content-type': 'image/png' } },
      )
    },
  }
  assert.deepEqual(await generateImage(http, connection, input, signal()), {
    image: { mime: 'image/png', data: PNG },
    revisedPrompt: null,
    usage: emptyUsage(),
    totalTokens: null,
  })
  assert.deepEqual(calls, ['https://images.example/v1/images/generations', 'https://cdn.example/image.png'])
})

test('download address rejection is never bypassed or retried', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch(url) {
      calls++
      if (calls === 1) return Response.json({ data: [{ url: 'http://127.0.0.1/image' }] })
      assert.equal(url, 'http://127.0.0.1/image')
      throw new ImageFailure('bad_request', 'Address blocked', 'not_dispatched')
    },
  }
  await assert.rejects(generateImage(http, connection, input, signal()), { dispatch: 'unknown', code: 'bad_request' })
  assert.equal(calls, 2)
})

test('redirects, unknown image bytes, MIME mismatch and oversized downloads fail', async () => {
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          return new Response(null, { status: 302, headers: { location: 'http://localhost/' } })
        },
      },
      connection,
      input,
      signal(),
    ),
    { code: 'upstream', dispatch: 'unknown' },
  )
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          return Response.json({ data: [{ b64_json: btoa('not an image') }] })
        },
      },
      connection,
      input,
      signal(),
    ),
    { code: 'bad_request', dispatch: 'unknown' },
  )
  for (const oversized of [false, true]) {
    let calls = 0
    const http: OutboundHttp = {
      async fetch() {
        calls++
        return calls === 1
          ? Response.json({ data: [{ url: 'https://cdn.example/image' }] })
          : new Response(
              oversized ? new Uint8Array(MAX_IMAGE_BYTES + 1) : Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0)),
              { headers: { 'content-type': oversized ? 'image/png' : 'image/jpeg' } },
            )
      },
    }
    await assert.rejects(generateImage(http, connection, input, signal()), {
      code: 'bad_request',
      details: oversized
        ? { field: 'images', reason: 'size', limit: MAX_IMAGE_BYTES }
        : { field: 'images', reason: 'mime' },
    })
    assert.equal(calls, 2)
  }
})

test('stream failure after successful status never advances key', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch() {
      calls++
      return new Response(
        new ReadableStream<Uint8Array>({
          start(controller) {
            controller.error(new Error('disconnected'))
          },
        }),
      )
    },
  }
  await assert.rejects(generateImage(http, connection, input, signal()), { code: 'network', dispatch: 'unknown' })
  assert.equal(calls, 1)
})

test('one 120s total deadline covers fallback and stalled body', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] })
  const first = Promise.withResolvers<Response>()
  const secondStarted = Promise.withResolvers<void>()
  let calls = 0
  const signals: (AbortSignal | null | undefined)[] = []
  const http: OutboundHttp = {
    async fetch(_url, init) {
      calls++
      signals.push(init.signal)
      if (calls === 1) return first.promise
      secondStarted.resolve()
      return new Response(new ReadableStream<Uint8Array>())
    },
  }
  const pending = generateImage(http, connection, input, signal())
  const assertion = assert.rejects(pending, { code: 'timeout', dispatch: 'unknown' })
  context.mock.timers.tick(IMAGE_TIMEOUT_MS - 1)
  first.resolve(new Response(null, { status: 401 }))
  await secondStarted.promise
  context.mock.timers.tick(1)
  await assertion
  assert.equal(signals[1]?.aborted, true)
  assert.equal(calls, 2)
})

test('cancellation propagates with unknown fact even if injected transport stalls', async () => {
  const controller = new AbortController()
  const gate = Promise.withResolvers<Response>()
  const signals: (AbortSignal | null | undefined)[] = []
  const http: OutboundHttp = {
    async fetch(_url, init) {
      signals.push(init.signal)
      return gate.promise
    },
  }
  const pending = generateImage(http, connection, input, controller.signal)
  controller.abort()
  await assert.rejects(pending, { code: 'cancelled', dispatch: 'unknown' })
  assert.equal(signals[0]?.aborted, true)
  gate.resolve(success())
})

test('strict base validation rejects fields, MIME, nonstandard base64 and oversize', () => {
  assert.doesNotThrow(() => checkImages([{ mime: 'image/png', data: PNG }]))
  for (const value of [
    { mime: 'image/svg+xml', data: PNG },
    { mime: 'image/png', data: 'Zg=' },
    { mime: 'image/png', data: 'Zh==' },
    { mime: 'image/png', data: PNG, injected: true },
    { mime: 'image/png', data: 'A'.repeat(Math.ceil(MAX_IMAGE_BYTES / 3) * 4 + 4) },
  ])
    assert.throws(() => checkImages([value]))
})

test('GPT quality and PNG mask are explicit multipart options and usage stays reported', async () => {
  const image = { mime: 'image/png', data: PNG }
  const result = await generateImage(
    {
      async fetch(_url, init) {
        assert.ok(init.body instanceof FormData)
        assert.equal(init.body.get('quality'), 'high')
        assert.equal(init.body.get('output_format'), 'png')
        assert.equal(init.body.get('background'), 'opaque')
        assert.equal(init.body.has('response_format'), false)
        assert.equal((init.body.get('mask') as File).type, 'image/png')
        return Response.json({
          data: [{ b64_json: PNG, revised_prompt: 'A revised tree' }],
          usage: {
            input_tokens: 12,
            output_tokens: 8,
            input_tokens_details: { cached_tokens: 3 },
          },
        })
      },
    },
    { ...connection, modelId: 'gpt-image-2' },
    { ...input, quality: 'high', images: [image], mask: image },
    signal(),
  )
  assert.equal(result.revisedPrompt, 'A revised tree')
  assert.deepEqual(result.usage, { ...emptyUsage(), inputTokens: 12, outputTokens: 8, cacheReadTokens: 3 })
})

test('generic and SiliconFlow reject unsupported options before dispatch', async () => {
  const http: OutboundHttp = {
    async fetch() {
      assert.fail('must not dispatch')
    },
  }
  for (const request of [
    { ...input, quality: 'medium' as const },
    { ...input, images: [{ mime: 'image/png', data: PNG }], mask: { mime: 'image/png', data: PNG } },
  ])
    await assert.rejects(generateImage(http, connection, request, signal()), {
      code: 'unsupported',
      dispatch: 'not_dispatched',
    })
  for (const limits of [{ timeoutMs: 180_001 }, { maxImageBytes: 10 * 1024 * 1024 + 1 }])
    await assert.rejects(generateImage(http, connection, input, signal(), limits), {
      code: 'bad_request',
      dispatch: 'not_dispatched',
    })
  assert.equal(imageCapabilities(connection, { profile: 'gpt-image' }).quality, true)
  await assert.rejects(
    generateImage(
      http,
      { ...connection, baseUrl: 'https://api.siliconflow.cn/v1' },
      { ...input, images: [{ mime: 'image/png', data: PNG }] },
      signal(),
    ),
    { code: 'unsupported', dispatch: 'not_dispatched' },
  )
})

test('SiliconFlow uses image_size and preserves usage through octet-stream download', async () => {
  let calls = 0
  const result = await generateImage(
    {
      async fetch(_url, init) {
        if (++calls === 1) {
          assert.deepEqual(JSON.parse(String(init.body)), {
            model: 'image-model',
            prompt: 'Draw a tree',
            image_size: '1024x1024',
          })
          return Response.json({
            images: [{ url: 'https://cdn.example/tree' }],
            usage: { input_tokens: 7, total_tokens: 19 },
          })
        }
        assert.equal(new Headers(init.headers).has('authorization'), false)
        return new Response(
          Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0)),
          { headers: { 'content-type': 'application/octet-stream' } },
        )
      },
    },
    { ...connection, imageBaseUrl: 'https://api.siliconflow.com/v1' },
    input,
    signal(),
  )
  assert.deepEqual(result.image, { mime: 'image/png', data: PNG })
  assert.deepEqual(result.usage, { ...emptyUsage(), inputTokens: 7 })
  assert.equal(result.totalTokens, 19)
})

test('successful malformed and multiple-image responses are unknown, never retried', async () => {
  for (const data of [
    [],
    [{ b64_json: PNG }, { b64_json: PNG }],
    [{ b64_json: PNG, url: 'https://cdn.example/x' }],
    [{ b64_json: 'iVBORw0KGgo=' }],
  ]) {
    let calls = 0
    await assert.rejects(
      generateImage(
        {
          async fetch() {
            calls++
            return Response.json({ data })
          },
        },
        connection,
        input,
        signal(),
      ),
      { dispatch: 'unknown' },
    )
    assert.equal(calls, 1)
  }
})

test('structural validation checks dimensions, alpha and complete PNG/GIF containers', () => {
  const bytes = Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0))
  assert.deepEqual(validateImageBytes(bytes), { mime: 'image/png', width: 1, height: 1, alpha: true })
  assert.throws(() => validateImageBytes(bytes.subarray(0, bytes.length - 1)))
  assert.throws(() => validateImageBytes(new Uint8Array([...bytes, 0])))
  const gif = Uint8Array.from(atob('R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw=='), (c) => c.charCodeAt(0))
  assert.equal(validateImageBytes(gif).mime, 'image/gif')
  assert.throws(() => validateImageBytes(gif.subarray(0, gif.length - 1)))
})

test('custom output bound leaves basic attachment validation unchanged', async () => {
  const image = { mime: 'image/png', data: PNG }
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          return success()
        },
      },
      connection,
      input,
      signal(),
      { maxImageBytes: 16 },
    ),
    { dispatch: 'unknown', code: 'bad_request' },
  )
  const huge = { ...image, data: 'A'.repeat(Math.ceil((MAX_IMAGE_BYTES + 1) / 3) * 4) }
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          assert.fail('must not dispatch')
        },
      },
      connection,
      { ...input, images: [huge] },
      signal(),
      { maxImageBytes: 10 * 1024 * 1024 },
    ),
    { dispatch: 'not_dispatched', code: 'bad_request' },
  )
})

test('missing download MIME is sniffed and one explicit larger output limit is honored', async () => {
  const original = Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0))
  const padding = MAX_IMAGE_BYTES
  const bytes = new Uint8Array(original.length + padding + 12)
  bytes.set(original.subarray(0, original.length - 12))
  const offset = original.length - 12
  new DataView(bytes.buffer).setUint32(offset, padding)
  bytes.set(new TextEncoder().encode('tEXt'), offset + 4)
  bytes.set(original.subarray(original.length - 12), bytes.length - 12)
  let calls = 0
  const result = await generateImage(
    {
      async fetch() {
        return ++calls === 1 ? Response.json({ data: [{ url: 'https://cdn.example/large' }] }) : new Response(bytes)
      },
    },
    connection,
    input,
    signal(),
    { maxImageBytes: 10 * 1024 * 1024 },
  )
  assert.equal(result.image.mime, 'image/png')
  assert.equal(atob(result.image.data).length, bytes.length)
  const image = { mime: 'image/png', data: result.image.data }
  assert.throws(() => checkImages([image]))
  await generateImage(
    {
      async fetch(_url, init) {
        assert.ok(init.body instanceof FormData)
        assert.equal((init.body.get('image') as File).size, bytes.length)
        return success()
      },
    },
    connection,
    { ...input, images: [image] },
    signal(),
    { maxImageBytes: 10 * 1024 * 1024 },
  )
})

test('mask dimensions, MIME, alpha and missing references are validated before dispatch', async () => {
  const image = { mime: 'image/png', data: PNG }
  const bytes = Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0))
  new DataView(bytes.buffer).setUint32(16, 2)
  const mismatched = { ...image, data: btoa(String.fromCharCode(...bytes)) }
  const opaque = Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0))
  opaque[25] = 2
  for (const mask of [
    mismatched,
    { ...image, mime: 'image/jpeg' },
    { ...image, data: btoa(String.fromCharCode(...opaque)) },
  ])
    await assert.rejects(
      generateImage(
        {
          async fetch() {
            assert.fail('must not dispatch')
          },
        },
        { ...connection, modelId: 'gpt-image-2' },
        { ...input, images: [image], mask },
        signal(),
      ),
      { dispatch: 'not_dispatched', code: 'bad_request' },
    )
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          assert.fail('must not dispatch')
        },
      },
      { ...connection, modelId: 'gpt-image-2' },
      { ...input, mask: image },
      signal(),
    ),
    { dispatch: 'not_dispatched', code: 'unsupported' },
  )
})

test('custom deadline expires once across a stalled generation', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] })
  const pending = generateImage({ fetch: () => new Promise<Response>(() => {}) }, connection, input, signal(), {
    timeoutMs: 10,
  })
  const assertion = assert.rejects(pending, { code: 'timeout', dispatch: 'unknown' })
  context.mock.timers.tick(10)
  await assertion
})

test('GPT generation sends quality without response_format and unknown names stay generic', async () => {
  await generateImage(
    {
      async fetch(_url, init) {
        assert.deepEqual(JSON.parse(String(init.body)), {
          model: 'private-image-model',
          prompt: 'Draw a tree',
          size: '1024x1024',
          n: 1,
          output_format: 'png',
          background: 'opaque',
          quality: 'medium',
        })
        return success()
      },
    },
    { ...connection, modelId: 'private-image-model' },
    { ...input, quality: 'medium' },
    signal(),
    { profile: 'gpt-image' },
  )
  assert.equal(imageCapabilities({ ...connection, modelId: 'gpt-image-future' }).profile, 'generic')
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          assert.fail('must not dispatch')
        },
      },
      { ...connection, baseUrl: 'not a url' },
      input,
      signal(),
    ),
    { dispatch: 'not_dispatched', code: 'bad_request' },
  )
})

test('exhausted credential responses retain rejected dispatch and public error code', async () => {
  for (const [status, code] of [
    [401, 'unauthorized'],
    [403, 'unauthorized'],
    [429, 'rate_limited'],
  ] as const) {
    let calls = 0
    await assert.rejects(
      generateImage(
        {
          async fetch() {
            calls++
            return new Response('', { status })
          },
        },
        { ...connection, keys: ['one'] },
        input,
        signal(),
      ),
      { dispatch: 'rejected', code, details: { status } },
    )
    assert.equal(calls, 1)
  }
})
