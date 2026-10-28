import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createHash, randomBytes } from 'node:crypto'

const local = {}
try {
  for (const line of (await readFile(new URL('../.dev.vars', import.meta.url), 'utf8')).split(/\r?\n/u)) {
    const split = line.indexOf('=')
    if (split > 0) local[line.slice(0, split)] = JSON.parse(line.slice(split + 1))
  }
} catch (error) { if (error.code !== 'ENOENT') throw error }
const option = name => process.env[name] ?? local[name]
const origin = option('SMOKE_ORIGIN') ?? 'http://127.0.0.1:8787'
const token = option('SMOKE_TOKEN'), token2 = option('SMOKE_TOKEN2')
assert.ok(token && token2, 'Configure both SMOKE_TOKEN and SMOKE_TOKEN2')
const providerUrl = option('SMOKE_PROVIDER_URL'), modelId = option('SMOKE_MODEL_ID')
assert.ok(providerUrl && modelId, 'SMOKE_PROVIDER_URL and SMOKE_MODEL_ID must point to a real tool-capable model service; the Worker never fabricates responses')
const headers = bearer => ({ Authorization: `Bearer ${bearer}`, Origin: origin, 'Content-Type': 'application/json' })
async function request(path, { method = 'GET', body, bearer = token, status = 200 } = {}) {
  const response = await fetch(new URL(path, origin), { method, headers: headers(bearer), ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(240000) })
  assert.equal(response.status, status, `${method} ${path}: ${response.status} ${status === response.status ? '' : await response.text()}`)
  return response
}
const health = await (await request('/health')).json()
assert.equal(health.storage, 'd1+r2')
await request('/api/v1/providers', { bearer: 'invalid-token', status: 401 })
const own = await (await request('/api/v1/host/markers')).json()
const foreign = await (await request('/api/v1/host/markers', { bearer: token2 })).json()
assert.notEqual(own.marker, foreign.marker)
const providers = await (await request('/api/v1/providers', { method: 'POST', body: {
  name: `Smoke ${randomBytes(8).toString('hex')}`, enabled: true,
  endpoints: [{ slug: 'primary', baseUrl: providerUrl, protocol: option('SMOKE_PROTOCOL') ?? 'openai-chat', models: [{ id: modelId, name: null, groupName: null, modelType: 'chat', typeSource: 'user', contextWindow: 32000, maxOutput: 4096, canVision: false, supportsTools: true, nativeWebSearch: false, pricing: null, reasoning: null }] }],
} })).json()
const endpoint = providers.at(-1).endpoints[0]
if (option('SMOKE_PROVIDER_KEY')) await request('/api/v1/providers/keys', { method: 'POST', body: { endpointId: endpoint.id, key: option('SMOKE_PROVIDER_KEY') } })
const runInput = input => ({ purpose: 'chat', model: { endpointId: endpoint.id, modelId }, input, skills: [], language: 'en', regenerate: false, permissionMode: 'default' })
async function consume(accepted, approve) {
  const response = await request(accepted.events)
  const reader = response.body.getReader(), decoder = new TextDecoder(), events = []
  let pending = ''
  try {
    while (true) {
      const chunk = await reader.read()
      if (chunk.done) break
      pending += decoder.decode(chunk.value, { stream: true }).replaceAll('\r\n', '\n')
      let end
      while ((end = pending.indexOf('\n\n')) !== -1) {
        const frame = pending.slice(0, end); pending = pending.slice(end + 2)
        const payload = frame.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n')
        if (!payload) continue
        const event = JSON.parse(payload); events.push(event)
        if (event.kind === 'approval_required') await request(`/api/v1/runs/${accepted.accepted.runId}/approval`, { method: 'POST', body: { callId: event.callId, decision: approve ? 'allow' : 'deny' } })
      }
    }
  } finally { reader.releaseLock() }
  assert.equal(events.filter(event => event.kind === 'end').length, 1)
  assert.equal(events.at(-1).status, 'completed', 'Real upstream must complete the run')
  return events
}
const read = await (await request('/api/v1/runs', { method: 'POST', body: runInput('Call read_marker exactly once now. Return its exact marker value without alteration. Do not guess the marker.'), status: 202 })).json()
await request(`/api/v1/runs/${read.accepted.runId}`, { bearer: token2, status: 404 })
const readEvents = await consume(read, false)
assert.ok(readEvents.some(event => event.kind === 'tool_result' && event.outcome === 'ok' && JSON.stringify(event).includes(own.marker)), 'Model must actually call actor-scoped read_marker')
const value = randomBytes(24).toString('hex')
const write = await (await request('/api/v1/runs', { method: 'POST', body: runInput(`Call write_marker exactly once with value ${value}. Wait for approval, then report success. Do not call any other tool.`), status: 202 })).json()
const writeEvents = await consume(write, true)
assert.ok(writeEvents.some(event => event.kind === 'approval_required' && event.name === 'write_marker'))
const saved = await (await request('/api/v1/host/markers')).json()
assert.equal(saved.written.value, value)
const file = await (await request(`/api/v1/files/${saved.written.fileId}`)).json()
assert.equal(file.value, value)
await request(`/api/v1/files/${saved.written.fileId}`, { bearer: token2, status: 404 })
const history = await (await request(`/api/v1/conversations/${read.accepted.conversationId}`)).json()
assert.ok(history.turns.some(turn => turn.id === read.accepted.turnId && turn.status === 'completed'))
const unused = await (await request('/api/v1/runs', { method: 'POST', body: runInput('Reply hello.'), status: 202 })).json()
await request(`/api/v1/runs/${unused.accepted.runId}/cancel`, { method: 'POST', body: {} })
assert.equal((await (await request(`/api/v1/runs/${unused.accepted.runId}`)).json()).status, 'cancelled')
const replay = await fetch(new URL(read.events, origin), { headers: { ...headers(token), 'Last-Event-ID': '1' } })
assert.equal(replay.status, 409)
if (origin.startsWith('https:')) {
  await request('/api/v1/mcp/settings', { method: 'PUT', body: { enabled: true, enabledScopes: ['markers:read', 'markers:write'] } })
  const registration = await fetch(`${origin}/oauth/register`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ client_name: 'Real smoke client', redirect_uris: ['http://127.0.0.1:9011/callback'], grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], token_endpoint_auth_method: 'none' }) })
  assert.equal(registration.status, 201)
  const client = await registration.json()
  const verifier = randomBytes(32).toString('base64url'), state = randomBytes(16).toString('hex')
  const query = new URLSearchParams({ client_id: client.client_id, redirect_uri: 'http://127.0.0.1:9011/callback', response_type: 'code', code_challenge_method: 'S256', code_challenge: createHash('sha256').update(verifier).digest('base64url'), scope: 'markers:read', state, resource: `${origin}/mcp` })
  const consent = await request(`/oauth/authorize?${query}`)
  const csrf = /name="csrf" value="([^"]+)"/u.exec(await consent.text())?.[1]
  assert.ok(csrf)
  const granted = await fetch(`${origin}/oauth/authorize?${query}`, { method: 'POST', redirect: 'manual', headers: { Authorization: `Bearer ${token}`, Origin: origin, 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ csrf, decision: 'approve' }) })
  assert.equal(granted.status, 302)
  const callback = new URL(granted.headers.get('location')); assert.equal(callback.searchParams.get('state'), state)
  const exchange = await fetch(`${origin}/oauth/token`, { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ grant_type: 'authorization_code', client_id: client.client_id, redirect_uri: 'http://127.0.0.1:9011/callback', code: callback.searchParams.get('code'), code_verifier: verifier, resource: `${origin}/mcp` }) })
  assert.equal(exchange.status, 200)
  const access = await exchange.json()
  const mcp = await request('/mcp', { method: 'POST', bearer: access.access_token, body: { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'read_marker', arguments: {} } } })
  assert.equal((await mcp.json()).result.structuredContent.marker, own.marker)
  const denied = await (await request('/mcp', { method: 'POST', bearer: access.access_token, body: { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'write_marker', arguments: { value: 'must-not-write' } } } })).json()
  assert.ok(denied.error)
}
console.log('Smoke passed: real model/tools, approval, actor isolation, D1 history, R2 effect, cancellation, replay rejection' + (origin.startsWith('https:') ? ', OAuth PKCE and MCP scope enforcement.' : '. HTTPS is required to exercise public MCP/OAuth.'))
