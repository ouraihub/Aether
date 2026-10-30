import assert from 'node:assert/strict'
import test from 'node:test'
import {
  ASSERTIONS,
  checkExecutable,
  checkKnownDrift,
  checkRunnerOutput,
  evaluate,
  firstDifference,
  judgeScenarios,
  normalize,
} from './scenario-check.mjs'

const executable = Object.keys(ASSERTIONS)
const accepted = [{ runId: '<run>', conversationId: null, turnId: null }]
const snapshot = (fields) => [
  { label: 'final', value: { runId: '<run>', conversationId: null, turnId: null, cancelRequested: false, ...fields } },
]
const usage = {
  kind: 'usage',
  inputTokens: 1,
  outputTokens: 1,
  cacheReadTokens: null,
  cacheWriteTokens: null,
  reasoningTokens: null,
  cost: null,
}
const text = (value) => ({ kind: 'text_delta', text: value })
const reasoning = (value) => ({ kind: 'reasoning_delta', text: value })
const toolCall = (callId) => ({ kind: 'tool_call', callId, name: 'read_note', source: 'host' })
const toolResult = (callId) => ({ kind: 'tool_result', callId, outcome: 'ok', summary: 'note contents' })

/** Observations satisfying every assertion, shaped like runner output. */
const passing = () => ({
  'acceptance-response-race': {
    accepted,
    events: [
      { kind: 'round_start', round: 1 },
      reasoning('Let me '),
      reasoning('think.'),
      text('Hel'),
      text('lo '),
      text('world'),
      usage,
      { kind: 'end', status: 'completed' },
    ],
    snapshots: snapshot({ status: 'completed' }),
    replies: [],
    modelRequests: 1,
    dispatches: [],
    marks: ['backend-accepted', 'model-released', 'end-received', 'accepted-delivered'],
  },
  'tool-approval-allow': {
    accepted,
    events: [
      { kind: 'round_start', round: 1 },
      toolCall('call-approve'),
      { kind: 'approval_required', callId: 'call-approve', name: 'read_note', argsPreview: 'x'.repeat(2000) },
      toolResult('call-approve'),
      { kind: 'round_start', round: 2 },
      text('done'),
      { kind: 'end', status: 'completed' },
    ],
    snapshots: snapshot({ status: 'completed' }),
    replies: [
      { op: 'approve', status: 'ok' },
      { op: 'approve-again', status: 'conflict', reason: 'already_decided' },
    ],
    modelRequests: 2,
    dispatches: ['call-approve'],
    marks: ['approval-required', 'dispatches-before-approval:0', 'end-received'],
  },
  'receiver-send-failure': {
    accepted,
    events: [{ kind: 'round_start', round: 1 }, text('first')],
    snapshots: snapshot({ status: 'cancelled', cancelRequested: true }),
    replies: [],
    modelRequests: 1,
    dispatches: [],
    marks: ['first-delta', 'receiver-closed', 'model-released', 'terminal'],
  },
  'round-exhaustion': {
    accepted,
    events: [
      { kind: 'round_start', round: 1 },
      toolCall('call-r1'),
      toolResult('call-r1'),
      { kind: 'round_start', round: 2 },
      toolCall('call-r2'),
      toolResult('call-r2'),
      { kind: 'end', status: 'completed', exhausted: true },
    ],
    snapshots: snapshot({ status: 'completed', exhausted: true }),
    replies: [],
    modelRequests: 2,
    dispatches: ['call-r1', 'call-r2'],
    marks: ['end-received'],
  },
})

const end = (o) => o.events.find((event) => event.kind === 'end')
const final = (o) => o.snapshots[0].value
/** One planted violation per check id; each mutates a passing observation. */
const violations = {
  'acceptance-response-race': {
    'marks-order': (o) => o.marks.reverse(),
    'accepted-ids-null': (o) => (o.accepted = [{ ...accepted[0], conversationId: '<conversation>' }]),
    'text-joined': (o) => (o.events[4] = text('LO ')),
    'reasoning-joined': (o) => o.events.splice(2, 1),
    'final-completed': (o) => (final(o).status = 'failed'),
    'end-once-last': (o) => o.events.push({ kind: 'round_start', round: 2 }),
    'end-completed': (o) => (end(o).status = 'cancelled'),
  },
  'tool-approval-allow': {
    'tool-call-before-approval': (o) => o.events.splice(1, 2, o.events[2], o.events[1]),
    'args-preview-limit': (o) => (o.events[2].argsPreview = 'x'.repeat(2001)),
    'dispatches-before-approval': (o) => (o.marks[1] = 'dispatches-before-approval:1'),
    dispatches: (o) => o.dispatches.push('call-approve'),
    'approve-ok': (o) => (o.replies[0].status = 'conflict'),
    'approve-again-rejected': (o) => (o.replies[1] = { op: 'approve-again', status: 'ok' }),
    'tool-result-ok': (o) => (o.events[3].outcome = 'error'),
    'end-once-last': (o) => o.events.splice(-1, 0, { kind: 'end', status: 'completed' }),
    'end-completed': (o) => (end(o).status = 'failed'),
  },
  'receiver-send-failure': {
    'no-second-delta': (o) => o.events.push(text('sec'), text('ond')),
    'no-end': (o) => o.events.push({ kind: 'end', status: 'cancelled' }),
    'final-cancelled': (o) => (final(o).cancelRequested = false),
    'dispatches-empty': (o) => o.dispatches.push('call-side'),
    'model-requests-1': (o) => (o.modelRequests = 2),
  },
  'round-exhaustion': {
    'model-requests-2': (o) => (o.modelRequests = 3),
    dispatches: (o) => o.dispatches.pop(),
    'end-once-last': (o) => o.events.pop(),
    'end-completed': (o) => delete end(o).exhausted,
    'final-completed': (o) => (final(o).exhausted = false),
  },
}

const runner = (backend, scenarios = passing()) => ({ backend, scenarios })
const judge = (knownDrift, native = passing(), worker = passing()) =>
  judgeScenarios({ executable, knownDrift, scenarios: { native, worker } })

test('passing observations satisfy every assertion and compare equal', () => {
  for (const [id, observation] of Object.entries(passing())) assert.deepEqual(evaluate(id, observation), [], id)
  assert.deepEqual(judge([]), { failures: [], drifts: [] })
})

test('every assertion catches its planted violation', () => {
  assert.deepEqual(Object.keys(violations), executable)
  for (const id of executable) {
    assert.deepEqual(Object.keys(violations[id]).sort(), Object.keys(ASSERTIONS[id]).sort(), id)
    for (const [check, plant] of Object.entries(violations[id])) {
      const observation = passing()[id]
      plant(observation)
      const failed = evaluate(id, observation).map((failure) => failure.check)
      assert.ok(failed.includes(check), `${id} ${check}: failed ${JSON.stringify(failed)}`)
    }
  }
})

test('argsPreview counts Unicode characters, not UTF-16 units', () => {
  const observation = passing()['tool-approval-allow']
  observation.events[2].argsPreview = '😀'.repeat(2000)
  assert.deepEqual(evaluate('tool-approval-allow', observation), [])
  observation.events[2].argsPreview = '😀'.repeat(2001)
  assert.deepEqual(
    evaluate('tool-approval-allow', observation).map((failure) => failure.check),
    ['args-preview-limit'],
  )
})

test('first difference path is exact', () => {
  const left = normalize(passing()['tool-approval-allow'])
  const right = normalize(passing()['tool-approval-allow'])
  right.events[3].summary = 'other'
  assert.deepEqual(firstDifference(left, right), { path: '$.events[3].summary', left: 'note contents', right: 'other' })
  right.events[3].summary = 'note contents'
  right.events[1].callId = 'call-other'
  right.events[3].callId = 'call-other'
  assert.equal(firstDifference(left, right).path, '$.events[1].callId')
  assert.equal(firstDifference(left, normalize(passing()['tool-approval-allow'])), undefined)
  assert.deepEqual(firstDifference({ a: [1, 2] }, { a: [1] }), { path: '$.a[1]', left: 2, right: undefined })
  assert.deepEqual(firstDifference({ x: 1 }, { x: 1, 'cache-key': 2 }), {
    path: '$["cache-key"]',
    left: undefined,
    right: 2,
  })
  assert.equal(firstDifference({ a: null }, { a: {} }).path, '$.a')
  assert.equal(firstDifference({ a: [] }, { a: {} }).path, '$.a')
})

test('delta runs collapse regardless of chunking and interleaving, but keep position', () => {
  const base = passing()['acceptance-response-race']
  assert.deepEqual(normalize(base).events, [
    { kind: 'round_start', round: 1 },
    { kind: 'output', text: 'Hello world', reasoning: 'Let me think.' },
    { kind: 'end', status: 'completed' },
  ])
  const rechunked = passing()['acceptance-response-race']
  rechunked.events.splice(
    1,
    6,
    text('Hello'),
    reasoning('Let'),
    { kind: 'context', usage: {} },
    text(' world'),
    reasoning(' me think.'),
  )
  assert.equal(firstDifference(normalize(base), normalize(rechunked)), undefined)

  const tool = passing()['tool-approval-allow']
  const moved = passing()['tool-approval-allow']
  moved.events.splice(5, 1)
  moved.events.splice(1, 0, text('done'))
  assert.equal(firstDifference(normalize(tool), normalize(moved)).path, '$.events[1].kind')

  const split = passing()['acceptance-response-race']
  split.events.splice(4, 0, { kind: 'search_start' })
  assert.equal(firstDifference(normalize(base), normalize(split)).path, '$.events[1].text')
})

test('normalization drops accounting and masks MCP boundaries', () => {
  const observation = passing()['round-exhaustion']
  const wrap = (id) => `<<<TOOL_RESULT_${id} server="docs">>>\nx\n<<<END_TOOL_RESULT_${id}>>>`
  observation.events[2].summary = wrap('0123456789ab')
  observation.events.splice(3, 0, usage, { kind: 'context', usage: {} })
  const other = passing()['round-exhaustion']
  other.events[2].summary = wrap('ba9876543210')
  const trace = normalize(observation)
  assert.equal(trace.events[2].summary, '<<<TOOL_RESULT_<B> server="docs">>>\nx\n<<<END_TOOL_RESULT_<B>>>>')
  assert.ok(trace.events.every((event) => event.kind !== 'usage' && event.kind !== 'context'))
  assert.equal(firstDifference(trace, normalize(other)), undefined)
  assert.deepEqual(Object.keys(trace), [
    'accepted',
    'events',
    'final',
    'replies',
    'modelRequests',
    'dispatches',
    'marks',
  ])
})

test('runner output must contain exactly the executable scenario ids', () => {
  assert.deepEqual(Object.keys(checkRunnerOutput(runner('native'), 'native', executable)), executable)
  const unknown = runner('worker')
  unknown.scenarios['owner-isolation'] = passing()['round-exhaustion']
  assert.throws(() => checkRunnerOutput(unknown, 'worker', executable), /unknown scenario owner-isolation/)
  const missing = runner('worker')
  delete missing.scenarios['receiver-send-failure']
  assert.throws(() => checkRunnerOutput(missing, 'worker', executable), /missing scenario receiver-send-failure/)
  assert.throws(() => checkRunnerOutput(runner('worker'), 'native', executable), /backend is "worker"/)
  assert.throws(() => checkRunnerOutput({ ...runner('native'), extra: 1 }, 'native', executable), /expected/)
  const field = runner('native')
  field.scenarios['round-exhaustion'].expected = true
  assert.throws(() => checkRunnerOutput(field, 'native', executable), /unknown field expected/)
  const count = runner('native')
  count.scenarios['round-exhaustion'].modelRequests = '2'
  assert.throws(() => checkRunnerOutput(count, 'native', executable), /invalid modelRequests/)
})

test('executable ids must exist and match the assertions table', () => {
  const defined = [...executable, 'owner-isolation']
  assert.doesNotThrow(() => checkExecutable(executable, defined))
  assert.throws(() => checkExecutable([...executable, 'no-such-scenario'], defined), /is not defined/)
  assert.throws(() => checkExecutable([...executable, 'owner-isolation'], defined), /has no assertions/)
  assert.throws(() => checkExecutable(executable.slice(1), defined), /non-executable scenario/)
  assert.throws(() => checkExecutable([...executable, executable[0]], defined), /duplicate/)
  assert.throws(() => evaluate('no-such-scenario', passing()['round-exhaustion']), /no assertions/)
})

test('unlisted failures fail and listed drift is reported while it occurs', () => {
  const worker = passing()
  worker['round-exhaustion'].modelRequests = 3
  const unlisted = judge([], passing(), worker)
  assert.deepEqual(unlisted.failures, [
    'round-exhaustion [worker] model-requests-2: modelRequests is 3, expected 2',
    'round-exhaustion trace differs at $.modelRequests: native 2, worker 3',
  ])
  const listed = [
    { scenario: 'round-exhaustion', backend: 'worker', check: 'model-requests-2', note: 'tracked' },
    { scenario: 'round-exhaustion', path: '$.modelRequests', note: 'tracked' },
  ]
  assert.deepEqual(judge(listed, passing(), worker), {
    failures: [],
    drifts: [
      { drift: 'round-exhaustion [worker] model-requests-2', note: 'tracked', occurs: true },
      { drift: 'round-exhaustion trace $.modelRequests', note: 'tracked', occurs: true },
    ],
  })
})

test('listed drift that no longer occurs fails', () => {
  const fixed = judge([{ scenario: 'round-exhaustion', backend: 'native', check: 'dispatches', note: 'tracked' }])
  assert.deepEqual(fixed.failures, [
    'round-exhaustion [native] dispatches: listed drift no longer occurs; remove it from knownDrift',
  ])
  assert.deepEqual(fixed.drifts, [{ drift: 'round-exhaustion [native] dispatches', note: 'tracked', occurs: false }])
  // A trace drift listed at one path does not cover a difference that moved elsewhere.
  const worker = passing()
  worker['tool-approval-allow'].dispatches = ['call-approve', 'call-approve']
  worker['tool-approval-allow'].modelRequests = 3
  const moved = judge(
    [
      { scenario: 'tool-approval-allow', backend: 'worker', check: 'dispatches', note: 'tracked' },
      { scenario: 'tool-approval-allow', path: '$.dispatches[1]', note: 'tracked' },
    ],
    passing(),
    worker,
  )
  assert.deepEqual(moved.failures, [
    'tool-approval-allow trace differs at $.modelRequests: native 2, worker 3',
    'tool-approval-allow trace $.dispatches[1]: listed drift no longer occurs; remove it from knownDrift',
  ])
})

test('knownDrift entries must name real scenarios, backends and checks with a note', () => {
  for (const entry of [
    { scenario: 'owner-isolation', backend: 'native', check: 'dispatches', note: 'n' },
    { scenario: 'round-exhaustion', backend: 'browser', check: 'dispatches', note: 'n' },
    { scenario: 'round-exhaustion', backend: 'native', check: 'no-such-check', note: 'n' },
    { scenario: 'round-exhaustion', backend: 'native', check: 'dispatches', note: ' ' },
    { scenario: 'round-exhaustion', backend: 'native', check: 'dispatches' },
    { scenario: 'round-exhaustion', path: 'events[0]', note: 'n' },
    { scenario: 'round-exhaustion', path: '$.events', backend: 'native', note: 'n' },
  ]) {
    assert.throws(() => checkKnownDrift([entry], executable), undefined, JSON.stringify(entry))
  }
  const entry = { scenario: 'round-exhaustion', path: '$.events', note: 'n' }
  assert.throws(() => checkKnownDrift([entry, { ...entry }], executable), /duplicate/)
})
