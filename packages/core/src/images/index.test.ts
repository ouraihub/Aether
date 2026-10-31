import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { OutboundHttp } from '../ports.js'
import {
  checkImages,
  generateImage,
  IMAGE_TIMEOUT_MS,
  ImageFailure,
  MAX_IMAGE_BYTES,
  type ImageConnection,
  type ImageRequest,
} from './index.js'

const PNG = 'iVBORw0KGgo='
const connection: ImageConnection = {
  baseUrl: 'https://images.example/v1',
  modelId: 'image-model',
  keys: ['first-secret', 'second-secret'],
  headers: {
    Authorization: 'custom-secret',
    'Content-Type': 'application/json',
    'x-custom': 'yes',
  },
  canVision: true,
}
const input: ImageRequest = {
  prompt: '  Draw a tree  ',
  size: '1024x1024',
  images: [],
}
const signal = (): AbortSignal => new AbortController().signal
const success = (): Response => Response.json({ data: [{ b64_json: PNG }] })

test('generation uses explicit transport, trimmed prompt, headers, and no response_format', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch(url, init) {
      calls++
      assert.equal(url, 'https://images.example/v1/images/generations')
      assert.equal(init.redirect, 'manual')
      assert.equal(
        new Headers(init.headers).get('authorization'),
        'Bearer first-secret',
      )
      assert.equal(new Headers(init.headers).get('x-custom'), 'yes')
      assert.equal(
        init.body,
        JSON.stringify({
          model: 'image-model',
          prompt: 'Draw a tree',
          size: '1024x1024',
          n: 1,
        }),
      )
      return success()
    },
  }
  assert.deepEqual(await generateImage(http, connection, input, signal()), {
    mime: 'image/png',
    data: PNG,
  })
  assert.equal(calls, 1)
})

for (const count of [1, 2, 4])
  test(`editing ${count} references uses native multipart MIME and filenames`, async () => {
    const images = Array.from({ length: count }, () => ({
      mime: 'image/png',
      data: PNG,
    }))
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
        return keys.length === 1
          ? new Response('secret upstream body', { status })
          : success()
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
      dispatch: 'rejected',
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
  await assert.rejects(
    generateImage(http, connection, input, controller.signal),
    {
      code: 'cancelled',
      dispatch: 'not_dispatched',
    },
  )
  await assert.rejects(
    generateImage(http, connection, { ...input, prompt: ' ' }, signal()),
    {
      code: 'bad_request',
      dispatch: 'not_dispatched',
    },
  )
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
      {
        ...input,
        images: Array.from({ length: 5 }, () => ({
          mime: 'image/png',
          data: PNG,
        })),
      },
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
      if (calls.length === 1)
        return Response.json({
          data: [{ url: 'https://cdn.example/image.png' }],
        })
      assert.equal(new Headers(init.headers).has('authorization'), false)
      assert.equal(init.redirect, 'manual')
      return new Response(
        Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0)),
        { headers: { 'content-type': 'image/png' } },
      )
    },
  }
  assert.deepEqual(await generateImage(http, connection, input, signal()), {
    mime: 'image/png',
    data: PNG,
  })
  assert.deepEqual(calls, [
    'https://images.example/v1/images/generations',
    'https://cdn.example/image.png',
  ])
})

test('download address rejection is never bypassed or retried', async () => {
  let calls = 0
  const http: OutboundHttp = {
    async fetch(url) {
      calls++
      if (calls === 1)
        return Response.json({ data: [{ url: 'http://127.0.0.1/image' }] })
      assert.equal(url, 'http://127.0.0.1/image')
      throw new ImageFailure('bad_request', 'Address blocked', 'not_dispatched')
    },
  }
  await assert.rejects(generateImage(http, connection, input, signal()), {
    dispatch: 'unknown',
    code: 'bad_request',
  })
  assert.equal(calls, 2)
})

test('redirects, unknown image bytes, MIME mismatch and oversized downloads fail', async () => {
  await assert.rejects(
    generateImage(
      {
        async fetch() {
          return new Response(null, {
            status: 302,
            headers: { location: 'http://localhost/' },
          })
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
              oversized
                ? new Uint8Array(MAX_IMAGE_BYTES + 1)
                : Uint8Array.from(atob(PNG), (c) => c.charCodeAt(0)),
              {
                headers: {
                  'content-type': oversized ? 'image/png' : 'image/jpeg',
                },
              },
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
  await assert.rejects(generateImage(http, connection, input, signal()), {
    code: 'network',
    dispatch: 'unknown',
  })
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
  const assertion = assert.rejects(pending, {
    code: 'timeout',
    dispatch: 'unknown',
  })
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
    {
      mime: 'image/png',
      data: 'A'.repeat(Math.ceil(MAX_IMAGE_BYTES / 3) * 4 + 4),
    },
  ])
    assert.throws(() => checkImages([value]))
})
