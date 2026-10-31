import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createWorkerOutbound,
  createWorkerTargetCheck,
  type OutboundLimits,
} from './outbound.ts'
const limits: OutboundLimits = {
  firstByteMs: 40,
  idleMs: 30,
  totalMs: 100,
  maxResponseBytes: 16,
}
const origin = 'https://api.example.com'
test('remote requests fail closed without a trusted connect-time binding', async () => {
  const outbound = createWorkerOutbound({ allowedOrigins: [origin], limits })
  await assert.rejects(outbound.fetch(`${origin}/`, {}), {
    code: 'needs_config',
  })
  for (const url of [
    'http://api.example.com',
    'https://user:secret@api.example.com',
    'https://127.0.0.1',
    'https://[::1]',
    'https://169.254.169.254',
    'https://10.0.0.1',
    'https://[::ffff:127.0.0.1]',
    'https://other.example.com',
  ]) {
    await assert.rejects(outbound.fetch(url, {}), { code: 'bad_request' })
  }
})
test('gateway returns manual redirects without following; callers own per-hop authorization', async () => {
  let calls = 0
  const outbound = createWorkerOutbound({
    allowedOrigins: [origin],
    limits,
    gateway: {
      security: 'public-addresses-at-connect-time',
      binding: {
        async fetch(_input, init) {
          calls++
          assert.equal(init?.redirect, 'manual')
          return new Response(null, {
            status: 302,
            headers: { Location: 'http://127.0.0.1' },
          })
        },
      },
    },
  })
  const response = await outbound.fetch(`${origin}/`, {})
  assert.equal(response.status, 302)
  assert.equal(response.headers.get('Location'), 'http://127.0.0.1')
  assert.equal(calls, 1)
})
test('only an explicit local flag permits literal loopback transport', async () => {
  const url = 'http://127.0.0.1:9011'
  const outbound = createWorkerOutbound({
    allowedOrigins: [url],
    limits,
    allowLoopbackForTests: true,
    async localFetch() {
      return new Response('ok')
    },
  })
  assert.equal(await (await outbound.fetch(url, {})).text(), 'ok')
  const denied = createWorkerOutbound({
    allowedOrigins: [url],
    limits,
    async localFetch() {
      throw new Error('Must not run')
    },
  })
  await assert.rejects(denied.fetch(url, {}), { code: 'bad_request' })
})
test('first-byte timer aborts an upstream that never returns headers', async () => {
  let signal: AbortSignal | null | undefined
  const gate = Promise.withResolvers<Response>()
  const outbound = createWorkerOutbound({
    allowedOrigins: [origin],
    limits,
    gateway: {
      security: 'public-addresses-at-connect-time',
      binding: {
        fetch(_input, init) {
          signal = init?.signal
          return gate.promise
        },
      },
    },
  })
  await assert.rejects(outbound.fetch(origin, {}), { code: 'timeout' })
  assert.equal(signal?.aborted, true)
})
test('stream content limits, idle deadlines and cancellation close upstream', async () => {
  let cancelled = false
  const outbound = createWorkerOutbound({
    allowedOrigins: [origin],
    limits,
    gateway: {
      security: 'public-addresses-at-connect-time',
      binding: {
        async fetch() {
          return new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                controller.enqueue(new Uint8Array(17))
              },
              cancel() {
                cancelled = true
              },
            }),
          )
        },
      },
    },
  })
  await assert.rejects((await outbound.fetch(origin, {})).arrayBuffer(), {
    code: 'bad_request',
  })
  assert.equal(cancelled, true)
  const idle = createWorkerOutbound({
    allowedOrigins: [origin],
    limits,
    gateway: {
      security: 'public-addresses-at-connect-time',
      binding: {
        async fetch() {
          return new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                controller.enqueue(new Uint8Array([1]))
              },
            }),
          )
        },
      },
    },
  })
  await assert.rejects((await idle.fetch(origin, {})).arrayBuffer(), {
    code: 'timeout',
  })
})

test('private gateway permission is exact origin and port, never a global private bypass', async () => {
  const target = 'http://10.1.2.3:8080'
  let calls = 0
  const outbound = createWorkerOutbound({
    allowedOrigins: [target, 'http://10.1.2.4:8080'],
    limits,
    privateGateway: {
      security: 'explicit-private-targets-at-connect-time',
      allowedOrigins: [target],
      binding: {
        async fetch() {
          calls++
          return new Response('ok')
        },
      },
    },
  })
  assert.equal(await (await outbound.fetch(target, {})).text(), 'ok')
  await assert.rejects(outbound.fetch('http://10.1.2.4:8080', {}), {
    code: 'bad_request',
  })
  await assert.rejects(outbound.fetch('http://10.1.2.3:8081', {}), {
    code: 'bad_request',
  })
  assert.equal(calls, 1)
})

test('total deadline and caller cancellation stop even unconsumed response streams', async () => {
  const outbound = createWorkerOutbound({
    allowedOrigins: [origin],
    limits: { ...limits, firstByteMs: 1000, idleMs: 1000, totalMs: 20 },
    gateway: {
      security: 'public-addresses-at-connect-time',
      binding: {
        async fetch() {
          return new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                controller.enqueue(new Uint8Array([1]))
              },
            }),
          )
        },
      },
    },
  })
  await assert.rejects((await outbound.fetch(origin, {})).arrayBuffer(), {
    code: 'timeout',
  })
  const controller = new AbortController()
  const response = await outbound.fetch(origin, { signal: controller.signal })
  controller.abort()
  await assert.rejects(response.arrayBuffer(), { code: 'cancelled' })
})
test('target checker actually invokes the trusted DNS policy and never substitutes syntax-only success', async () => {
  let calls = 0
  const check = createWorkerTargetCheck({
    checker: {
      security: 'all-dns-addresses-preflight',
      async checkTarget() {
        calls++
        throw new Error('DNS rejected')
      },
    },
  })
  const signal = new AbortController().signal
  await assert.rejects(
    check(new URL('https://public.example.com'), signal),
    /DNS rejected/u,
  )
  assert.equal(calls, 1)
  await assert.rejects(check(new URL('https://10.0.0.1'), signal), {
    code: 'bad_request',
  })
  assert.equal(calls, 1)
})
