// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import type { RunEvent } from '@ouraihub/aether-contracts'
import { createWebPorts } from './index'
import type { WebPortsOptions } from './index'

const input = {
  purpose: 'chat',
  skills: [],
  input: 'hello',
  regenerate: false,
  language: 'en',
}
const accepted = { runId: 'run', conversationId: null, turnId: null }
const baseURL = 'https://app.example'
function required<T>(value: T | undefined): T {
  if (value === undefined) throw new Error('Missing test capability')
  return value
}
const deferred = <T>() => Promise.withResolvers<T>()
function events() {
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined
  const body = new ReadableStream<Uint8Array>({
    start(value) {
      controller = value
    },
  })
  return {
    response: new Response(body, {
      headers: { 'Content-Type': 'text/event-stream' },
    }),
    push: (text: string) =>
      required(controller).enqueue(new TextEncoder().encode(text)),
    close: () => required(controller).close(),
  }
}
function fixture(overrides: Partial<WebPortsOptions> = {}) {
  const stream = events()
  const calls: { path: string; init: RequestInit }[] = []
  const fetch: WebPortsOptions['fetch'] = async (url, init) => {
    const path = new URL(url).pathname
    calls.push({ path, init })
    if (path === '/api/v1/runs')
      return Response.json(
        { accepted, events: '/api/v1/runs/run/events' },
        { status: 202 },
      )
    if (path.endsWith('/events')) return stream.response
    if (path.endsWith('/cancel') || path.endsWith('/approval'))
      return Response.json({ ok: true })
    if (path === '/api/v1/runs/run')
      return Response.json({
        ...accepted,
        status: 'cancelled',
        cancelRequested: true,
      })
    return Response.json(
      { code: 'not_found', message: 'Missing' },
      { status: 404 },
    )
  }
  return {
    stream,
    calls,
    ports: createWebPorts({
      baseURL,
      fetch,
      csrf: 'csrf',
      chat: true,
      ...overrides,
    }),
  }
}

describe('web RunClient', () => {
  it(
    'accepts 202 before completion, parses fragmented CRLF SSE and ignores ' +
      'stale events after one end',
    async () => {
    const f = fixture()
    const received: RunEvent[] = []
    const done = deferred<void>()
    const chat = required(f.ports.chat)
    expect(
      await chat.start(input, (event) => {
        received.push(event)
        if (event.kind === 'end') done.resolve()
      }),
    ).toEqual(accepted)
    f.stream.push(
      ': ping\r\nevent: run_event\r\nid: 1\r\ndata: {"kind":"text_delta",' +
        '"text":"雪"}\r',
    )
    f.stream.push(
      '\n\r\nevent: run_event\r\ndata: {"kind":"end","status":"completed"}' +
        '\r\n\r\ndata: {"kind":"text_delta","text":"stale"}\n\n',
    )
    await done.promise
    expect(received).toEqual([
      { kind: 'text_delta', text: '雪' },
      { kind: 'end', status: 'completed' },
    ])
    expect(f.calls[0]?.init.signal).toBeUndefined()
    expect(new Headers(f.calls[0]?.init.headers).get('X-CSRF-Token')).toBe(
      'csrf',
    )
    expect(new Headers(f.calls[1]?.init.headers).get('Accept')).toBe(
      'text/event-stream',
    )
  })

  it('does not send an already aborted request', async () => {
    const f = fixture()
    const signal = AbortSignal.abort()
    await expect(
      required(f.ports.chat).start(input, () => undefined, { signal }),
    ).rejects.toMatchObject({
      code: 'cancelled',
    })
    expect(f.calls).toEqual([])
  })

  it(
    'retains abort intent before acceptance and cancels before opening ' +
      'subscription',
    async () => {
    const response = deferred<Response>()
    const order: string[] = []
    const stream = events()
    const fetch: WebPortsOptions['fetch'] = async (url) => {
      const path = new URL(url).pathname
      order.push(path)
      if (path === '/api/v1/runs') return response.promise
      if (path.endsWith('/cancel')) return Response.json({ ok: true })
      return stream.response
    }
    const chat = required(
      createWebPorts({ baseURL, fetch, csrf: 'csrf', chat: true }).chat,
    )
    const controller = new AbortController()
    const done = deferred<void>()
    const pending = chat.start(
      input,
      (event) => {
        if (event.kind === 'end') done.resolve()
      },
      { signal: controller.signal },
    )
    controller.abort()
    response.resolve(
      Response.json(
        { accepted, events: '/api/v1/runs/run/events' },
        { status: 202 },
      ),
    )
    await expect(pending).resolves.toEqual(accepted)
    stream.push('data: {"kind":"end","status":"cancelled"}\n\n')
    await done.promise
    expect(order).toEqual([
      '/api/v1/runs',
      '/api/v1/runs/run/cancel',
      '/api/v1/runs/run/events',
    ])
  })

  it(
    'requests cancellation after accepted without inventing a local end',
    async () => {
    const f = fixture()
    const controller = new AbortController()
    const received: RunEvent[] = []
    const done = deferred<void>()
    await required(f.ports.chat).start(
      input,
      (event) => {
        received.push(event)
        if (event.kind === 'end') done.resolve()
      },
      { signal: controller.signal },
    )
    controller.abort()
    expect(received).toEqual([])
    f.stream.push('data: {"kind":"end","status":"cancelled"}\n\n')
    await done.promise
    expect(
      f.calls.filter((call) => call.path.endsWith('/cancel')),
    ).toHaveLength(1)
    expect(received).toEqual([{ kind: 'end', status: 'cancelled' }])
  })

  it(
    'malformed SSE cancels and confirms snapshot instead of fabricating ' +
      'completion',
    async () => {
    const f = fixture()
    const received: RunEvent[] = []
    const done = deferred<void>()
    await required(f.ports.chat).start(input, (event) => {
      received.push(event)
      if (event.kind === 'end') done.resolve()
    })
    f.stream.push('data: {"kind":"end","status":"unknown"}\n\n')
    await done.promise
    expect(received).toEqual([{ kind: 'end', status: 'cancelled' }])
    expect(f.calls.map((call) => call.path)).toContain(
      '/api/v1/runs/run/cancel',
    )
    expect(f.calls.map((call) => call.path)).toContain('/api/v1/runs/run')
  })

  it(
    'reports an interrupted running stream without a false terminal event',
    async () => {
    const report = deferred<unknown>()
    const received: RunEvent[] = []
    const stream = events()
    const fetch: WebPortsOptions['fetch'] = async (url) => {
      const path = new URL(url).pathname
      if (path === '/api/v1/runs')
        return Response.json(
          { accepted, events: '/api/v1/runs/run/events' },
          { status: 202 },
        )
      if (path.endsWith('/events')) return stream.response
      if (path.endsWith('/cancel')) return Response.json({ ok: true })
      return Response.json({
        ...accepted,
        status: 'running',
        cancelRequested: true,
      })
    }
    const chat = required(
      createWebPorts({
        baseURL,
        fetch,
        csrf: 'x',
        chat: true,
        onStreamError: (_, error) => report.resolve(error),
      }).chat,
    )
    await chat.start(input, (event) => received.push(event))
    stream.close()
    await expect(report.promise).resolves.toMatchObject({
      code: 'network',
      details: { reason: 'stream_disconnected' },
    })
    expect(received).toEqual([])
  })

  it(
    'returns preaccept errors without subscribing or emitting a failure event',
    async () => {
    const fetch = vi
      .fn<WebPortsOptions['fetch']>()
      .mockImplementation(async () =>
        Response.json(
          {
            code: 'needs_config',
            message: 'Select a model',
            details: { reason: 'model_required' },
          },
          { status: 400 },
        ),
      )
    const onEvent = vi.fn()
    await expect(
      required(
        createWebPorts({ baseURL, fetch, csrf: 'x', chat: true }).chat,
      ).start(input, onEvent),
    ).rejects.toMatchObject({
      code: 'needs_config',
      details: { reason: 'model_required' },
    })
    expect(fetch).toHaveBeenCalledTimes(1)
    expect(onEvent).not.toHaveBeenCalled()
  })

  it(
    'failed subscription recycles the run and reports a missing snapshot ' +
      'without a false end',
    async () => {
    const report = deferred<unknown>()
    const onEvent = vi.fn()
    const paths: string[] = []
    const fetch: WebPortsOptions['fetch'] = async (url) => {
      const path = new URL(url).pathname
      paths.push(path)
      if (path === '/api/v1/runs')
        return Response.json(
          { accepted, events: '/api/v1/runs/run/events' },
          { status: 202 },
        )
      if (path.endsWith('/cancel')) return Response.json({ ok: true })
      return Response.json(
        { code: 'not_found', message: 'Not found' },
        { status: 404 },
      )
    }
    const chat = required(
      createWebPorts({
        baseURL,
        fetch,
        csrf: 'x',
        chat: true,
        onStreamError: (_, error) => report.resolve(error),
      }).chat,
    )
    await expect(chat.start(input, onEvent)).resolves.toEqual(accepted)
    await expect(report.promise).resolves.toMatchObject({ code: 'not_found' })
    expect(paths).toContain('/api/v1/runs/run/cancel')
    expect(onEvent).not.toHaveBeenCalled()
  })

  it(
    'rejects untrusted transport URLs and malformed acceptance data',
    async () => {
    const fetch: WebPortsOptions['fetch'] = async () =>
      Response.json(
        { accepted, events: 'https://evil.example/api/v1/runs/run/events' },
        { status: 202 },
      )
    await expect(
      required(
        createWebPorts({ baseURL, fetch, csrf: '', chat: true }).chat,
      ).start(input, () => undefined),
    ).rejects.toMatchObject({ code: 'upstream' })
  })
})

describe('web settings and conversation boundaries', () => {
  it('exposes independent conversation capability and no desktop', async () => {
    const calls: string[] = []
    const fetch: WebPortsOptions['fetch'] = async (url) => {
      calls.push(url)
      return Response.json([])
    }
    const ports = createWebPorts({
      baseURL,
      fetch,
      csrf: '',
      conversations: true,
    })
    expect(Object.keys(ports)).toEqual(['conversations'])
    expect(await required(ports.conversations).list('private scope')).toEqual(
      [],
    )
    expect(calls).toEqual([
      'https://app.example/api/v1/conversations?scope=private%20scope',
    ])
  })

  it(
    'rejects unknown response fields and preserves structured authorization ' +
      'errors',
    async () => {
    const fetch = vi.fn<WebPortsOptions['fetch']>()
    fetch
      .mockResolvedValueOnce(
        Response.json({ summary: {}, turns: [], secret: 'forbidden' }),
      )
      .mockResolvedValueOnce(
        Response.json(
          {
            code: 'forbidden',
            message: 'Denied',
            details: { reason: 'fresh_auth_required' },
          },
          { status: 403 },
        ),
      )
    const ports = createWebPorts({
      baseURL,
      fetch,
      csrf: 'x',
      conversations: true,
      providers: true,
    })
    await expect(required(ports.conversations).get('c')).rejects.toMatchObject({
      code: 'upstream',
    })
    await expect(
      required(ports.providers).revealKey('k', 'pw'),
    ).rejects.toMatchObject({
      code: 'forbidden',
      details: { reason: 'fresh_auth_required' },
    })
  })

  it(
    'maps settings writes to canonical routes with explicit CSRF and JSON ' +
      'bodies',
    async () => {
    const fetch = vi
      .fn<WebPortsOptions['fetch']>()
      .mockImplementation(async () => Response.json([]))
    const ports = createWebPorts({
      baseURL,
      fetch,
      csrf: async () => 'fresh',
      providers: true,
      skills: true,
      mcpClient: true,
    })
    await required(ports.providers).setEnabled('p/a', false)
    await required(ports.skills).reorder(['s'])
    await required(ports.mcpClient).remove('m')
    expect(
      fetch.mock.calls.map(([url, init]) => [
        new URL(url).pathname,
        init.method,
        init.body,
      ]),
    ).toEqual([
      ['/api/v1/providers/p%2Fa/enabled', 'POST', '{"enabled":false}'],
      ['/api/v1/skills/reorder', 'POST', '{"ids":["s"]}'],
      ['/api/v1/mcp/servers/m', 'DELETE', undefined],
    ])
    expect(
      fetch.mock.calls.every(
        ([, init]) => new Headers(init.headers).get('X-CSRF-Token') === 'fresh',
      ),
    ).toBe(true)
  })
})
