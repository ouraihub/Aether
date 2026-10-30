// Scenario-run checks for contracts-check (contracts/scenario-runs.md): runner output shape,
// per-scenario assertions, normalized traces, first-difference comparison and known drift.
// Pure functions over parsed runner output; schema validation and process handling stay in contracts-check.
import { isDeepStrictEqual } from 'node:util'

export const BACKENDS = ['native', 'worker']

const show = (value) => (value === undefined ? 'nothing' : JSON.stringify(value))
const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value)
const pick = (value, keys) => Object.fromEntries(keys.map((key) => [key, value?.[key]]))
const ofKind = (observation, kind) => observation.events.filter((event) => event.kind === kind)
const joined = (observation, kind) =>
  ofKind(observation, kind)
    .map((event) => event.text)
    .join('')
const snapshot = (observation, label) => observation.snapshots.find((entry) => entry.label === label)?.value
const reply = (observation, op) => observation.replies.find((entry) => entry.op === op)
const lastEnd = (observation) => ofKind(observation, 'end').at(-1)

/** `undefined` when equal, otherwise a diagnostic. */
const equal = (label, actual, expected) =>
  isDeepStrictEqual(actual, expected) ? undefined : `${label} is ${show(actual)}, expected ${show(expected)}`
/** Compares only `keys` of a value that must exist. */
const fields = (label, value, expected) =>
  value === undefined ? `no ${label}` : equal(label, pick(value, Object.keys(expected)), expected)

const endOnceLast = (observation) => {
  const ends = ofKind(observation, 'end')
  if (ends.length !== 1) return `${ends.length} end events, expected 1`
  const last = observation.events.at(-1)
  return last === ends[0] ? undefined : `end is not the last event; last is ${show(last?.kind)}`
}

/** The scenario-runs.md assertions table; check ids are stable and referenced by knownDrift. */
export const ASSERTIONS = {
  'acceptance-response-race': {
    'marks-order': (o) =>
      equal('marks', o.marks, ['backend-accepted', 'model-released', 'end-received', 'accepted-delivered']),
    'accepted-ids-null': (o) =>
      equal(
        'accepted ids',
        o.accepted.map((value) => pick(value, ['conversationId', 'turnId'])),
        [{ conversationId: null, turnId: null }],
      ),
    'text-joined': (o) => equal('joined text_delta', joined(o, 'text_delta'), 'Hello world'),
    'reasoning-joined': (o) => equal('joined reasoning_delta', joined(o, 'reasoning_delta'), 'Let me think.'),
    'final-completed': (o) => fields('snapshot final', snapshot(o, 'final'), { status: 'completed' }),
    'end-once-last': endOnceLast,
    'end-completed': (o) => fields('end', lastEnd(o), { status: 'completed' }),
  },
  'tool-approval-allow': {
    'tool-call-before-approval': (o) => {
      const call = o.events.findIndex((event) => event.kind === 'tool_call')
      const approval = o.events.findIndex((event) => event.kind === 'approval_required')
      if (call < 0 || approval < 0) return `tool_call at events[${call}], approval_required at events[${approval}]`
      return call < approval
        ? undefined
        : `tool_call at events[${call}] follows approval_required at events[${approval}]`
    },
    'args-preview-limit': (o) => {
      const lengths = ofKind(o, 'approval_required').map((event) => [...String(event.argsPreview)].length)
      if (lengths.length === 0) return 'no approval_required event'
      const over = lengths.find((length) => length > 2000)
      return over === undefined ? undefined : `argsPreview has ${over} characters, limit 2000`
    },
    'dispatches-before-approval': (o) =>
      equal(
        'dispatches-before-approval mark',
        o.marks.filter((mark) => mark.startsWith('dispatches-before-approval:')),
        ['dispatches-before-approval:0'],
      ),
    dispatches: (o) => equal('dispatches', o.dispatches, ['call-approve']),
    'approve-ok': (o) => fields('reply approve', reply(o, 'approve'), { status: 'ok' }),
    'approve-again-rejected': (o) => {
      const again = reply(o, 'approve-again')
      if (!again) return 'no reply approve-again'
      return ['conflict', 'not_found'].includes(again.status)
        ? undefined
        : `reply approve-again is ${show(again.status)}, expected conflict or not_found`
    },
    'tool-result-ok': (o) =>
      equal(
        'tool_result events',
        ofKind(o, 'tool_result').map((event) => pick(event, ['outcome', 'summary'])),
        [{ outcome: 'ok', summary: 'note contents' }],
      ),
    'end-once-last': endOnceLast,
    'end-completed': (o) => fields('end', lastEnd(o), { status: 'completed' }),
  },
  'receiver-send-failure': {
    'no-second-delta': (o) =>
      joined(o, 'text_delta').includes('second') ? 'text_delta "second" reached the closed receiver' : undefined,
    'no-end': (o) => (ofKind(o, 'end').length ? 'end reached the closed receiver' : undefined),
    'final-cancelled': (o) =>
      fields('snapshot final', snapshot(o, 'final'), { status: 'cancelled', cancelRequested: true }),
    'dispatches-empty': (o) => equal('dispatches', o.dispatches, []),
    'model-requests-1': (o) => equal('modelRequests', o.modelRequests, 1),
  },
  'round-exhaustion': {
    'model-requests-2': (o) => equal('modelRequests', o.modelRequests, 2),
    dispatches: (o) => equal('dispatches', o.dispatches, ['call-r1', 'call-r2']),
    'end-once-last': endOnceLast,
    'end-completed': (o) => fields('end', lastEnd(o), { status: 'completed', exhausted: true }),
    'final-completed': (o) => fields('snapshot final', snapshot(o, 'final'), { status: 'completed', exhausted: true }),
  },
}

/** `executable` must name distinct scenarios.json ids and exactly the scenarios with an assertions table. */
export function checkExecutable(executable, scenarioIds) {
  if (!Array.isArray(executable) || executable.length === 0)
    throw new Error('scenarios.json executable must be nonempty')
  if (new Set(executable).size !== executable.length) throw new Error('scenarios.json executable has duplicate ids')
  for (const id of executable)
    if (!scenarioIds.includes(id)) throw new Error(`executable scenario ${id} is not defined`)
  for (const id of Object.keys(ASSERTIONS)) {
    if (!executable.includes(id)) throw new Error(`assertions exist for non-executable scenario ${id}`)
  }
  for (const id of executable) if (!ASSERTIONS[id]) throw new Error(`executable scenario ${id} has no assertions`)
}

const OBSERVATION = {
  accepted: (value) => Array.isArray(value),
  events: (value) => Array.isArray(value),
  snapshots: (value) =>
    Array.isArray(value) &&
    value.every(
      (entry) =>
        isObject(entry) &&
        Object.keys(entry).length === 2 &&
        typeof entry.label === 'string' &&
        Object.hasOwn(entry, 'value'),
    ),
  replies: (value) =>
    Array.isArray(value) &&
    value.every(
      (entry) =>
        isObject(entry) &&
        Object.keys(entry).every((key) => ['op', 'status', 'reason'].includes(key)) &&
        typeof entry.op === 'string' &&
        typeof entry.status === 'string' &&
        (entry.reason === undefined || typeof entry.reason === 'string'),
    ),
  modelRequests: (value) => Number.isSafeInteger(value) && value >= 0,
  dispatches: (value) => Array.isArray(value) && value.every((entry) => typeof entry === 'string'),
  marks: (value) => Array.isArray(value) && value.every((entry) => typeof entry === 'string'),
}

/** Checks one runner document's envelope; scenario ids must be exactly `executable`. Returns its scenarios. */
export function checkRunnerOutput(output, backend, executable) {
  const label = `${backend} scenario output`
  if (!isObject(output) || Object.keys(output).some((key) => !['backend', 'scenarios'].includes(key))) {
    throw new Error(`${label}: expected {backend,scenarios}`)
  }
  if (output.backend !== backend) throw new Error(`${label}: backend is ${show(output.backend)}`)
  if (!isObject(output.scenarios)) throw new Error(`${label}: scenarios must be an object`)
  for (const id of Object.keys(output.scenarios)) {
    if (!executable.includes(id)) throw new Error(`${label}: unknown scenario ${id}`)
  }
  for (const id of executable) {
    const observation = output.scenarios[id]
    if (observation === undefined) throw new Error(`${label}: missing scenario ${id}`)
    if (!isObject(observation)) throw new Error(`${label} ${id}: observation must be an object`)
    for (const key of Object.keys(observation)) {
      if (!Object.hasOwn(OBSERVATION, key)) throw new Error(`${label} ${id}: unknown field ${key}`)
    }
    for (const [key, valid] of Object.entries(OBSERVATION)) {
      if (!valid(observation[key])) throw new Error(`${label} ${id}: invalid ${key}`)
    }
  }
  return output.scenarios
}

/** Every public value of one observation as `[contract, value, label]`, for schema validation. */
export function* publicValues(backend, id, observation) {
  for (const [index, value] of observation.accepted.entries()) {
    yield ['RunAccepted', value, `${backend} ${id} accepted[${index}]`]
  }
  for (const [index, value] of observation.events.entries()) {
    yield ['RunEvent', value, `${backend} ${id} events[${index}]`]
  }
  for (const [index, entry] of observation.snapshots.entries()) {
    yield ['RunSnapshot', entry.value, `${backend} ${id} snapshots[${index}] (${entry.label})`]
  }
}

/** Failed assertions of one observation as `{ check, message }`, in table order. */
export function evaluate(id, observation) {
  const checks = ASSERTIONS[id]
  if (!checks) throw new Error(`scenario ${id} has no assertions`)
  const failures = []
  for (const [check, assertion] of Object.entries(checks)) {
    const message = assertion(observation)
    if (message !== undefined) failures.push({ check, message })
  }
  return failures
}

const BOUNDARY = /(<<<(?:END_)?TOOL_RESULT_)[0-9a-f]{12}/g
const unbound = (value) => {
  if (typeof value === 'string') return value.replace(BOUNDARY, '$1<B>')
  if (Array.isArray(value)) return value.map(unbound)
  if (isObject(value)) return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, unbound(item)]))
  return value
}

/** The comparable trace: accounting events dropped, each maximal delta run collapsed, MCP boundaries `<B>`. */
export function normalize(observation) {
  const events = []
  for (const event of observation.events) {
    if (event.kind === 'usage' || event.kind === 'context') continue
    if (event.kind !== 'text_delta' && event.kind !== 'reasoning_delta') {
      events.push(event)
      continue
    }
    let output = events.at(-1)
    if (output?.kind !== 'output') {
      output = { kind: 'output', text: '', reasoning: '' }
      events.push(output)
    }
    output[event.kind === 'text_delta' ? 'text' : 'reasoning'] += event.text
  }
  return unbound({
    accepted: observation.accepted,
    events,
    final: snapshot(observation, 'final') ?? null,
    replies: observation.replies,
    modelRequests: observation.modelRequests,
    dispatches: observation.dispatches,
    marks: observation.marks,
  })
}

const step = (path, key) => (/^[A-Za-z_$][\w$]*$/.test(key) ? `${path}.${key}` : `${path}[${JSON.stringify(key)}]`)

/** First path where two JSON values differ (`$.events[3].text`) with both sides, or `undefined` when equal. */
export function firstDifference(left, right, path = '$') {
  if (Array.isArray(left) && Array.isArray(right)) {
    for (let index = 0; index < Math.max(left.length, right.length); index++) {
      const at = `${path}[${index}]`
      if (index >= left.length || index >= right.length) return { path: at, left: left[index], right: right[index] }
      const difference = firstDifference(left[index], right[index], at)
      if (difference) return difference
    }
    return undefined
  }
  if (isObject(left) && isObject(right)) {
    for (const key of new Set([...Object.keys(left), ...Object.keys(right)])) {
      const at = step(path, key)
      if (!Object.hasOwn(left, key) || !Object.hasOwn(right, key))
        return { path: at, left: left[key], right: right[key] }
      const difference = firstDifference(left[key], right[key], at)
      if (difference) return difference
    }
    return undefined
  }
  return left === right ? undefined : { path, left, right }
}

const describeDrift = (entry) =>
  entry.path === undefined
    ? `${entry.scenario} [${entry.backend}] ${entry.check}`
    : `${entry.scenario} trace ${entry.path}`

/** knownDrift entries are `{ scenario, backend, check, note }` or `{ scenario, path, note }` naming real checks. */
export function checkKnownDrift(knownDrift, executable) {
  if (!Array.isArray(knownDrift)) throw new Error('knownDrift must be an array')
  const seen = new Set()
  for (const [index, entry] of knownDrift.entries()) {
    const label = `knownDrift[${index}]`
    const keys = isObject(entry) ? Object.keys(entry).sort().join(',') : ''
    if (keys !== 'backend,check,note,scenario' && keys !== 'note,path,scenario') {
      throw new Error(`${label}: expected {scenario,backend,check,note} or {scenario,path,note}`)
    }
    if (!executable.includes(entry.scenario)) throw new Error(`${label}: unknown scenario ${show(entry.scenario)}`)
    if (typeof entry.note !== 'string' || entry.note.trim() === '') throw new Error(`${label}: note is required`)
    if (entry.path === undefined) {
      if (!BACKENDS.includes(entry.backend)) throw new Error(`${label}: unknown backend ${show(entry.backend)}`)
      if (!Object.hasOwn(ASSERTIONS[entry.scenario], entry.check)) {
        throw new Error(`${label}: unknown check ${show(entry.check)}`)
      }
    } else if (typeof entry.path !== 'string' || !entry.path.startsWith('$')) {
      throw new Error(`${label}: path must start with $`)
    }
    const key = describeDrift(entry)
    if (seen.has(key)) throw new Error(`${label}: duplicate drift ${key}`)
    seen.add(key)
  }
}

const clip = (value) => {
  const text = show(value)
  return text.length > 200 ? `${text.slice(0, 200)}…` : text
}

/**
 * Assertions on both backends plus trace comparison, reconciled with knownDrift: unlisted failures fail,
 * listed drift is reported each run, listed drift that no longer occurs fails.
 * `scenarios` maps backend → checked runner `scenarios`.
 */
export function judgeScenarios({ executable, knownDrift, scenarios }) {
  checkKnownDrift(knownDrift, executable)
  const failures = []
  const occurring = new Set()
  const settle = (matches, message) => {
    const index = knownDrift.findIndex(matches)
    if (index < 0) failures.push(message)
    else occurring.add(index)
  }
  for (const id of executable) {
    for (const backend of BACKENDS) {
      for (const { check, message } of evaluate(id, scenarios[backend][id])) {
        settle(
          (entry) => entry.scenario === id && entry.backend === backend && entry.check === check,
          `${id} [${backend}] ${check}: ${message}`,
        )
      }
    }
    const difference = firstDifference(normalize(scenarios.native[id]), normalize(scenarios.worker[id]))
    if (difference) {
      settle(
        (entry) => entry.scenario === id && entry.path === difference.path,
        `${id} trace differs at ${difference.path}: native ${clip(difference.left)}, worker ${clip(difference.right)}`,
      )
    }
  }
  const drifts = knownDrift.map((entry, index) => ({
    drift: describeDrift(entry),
    note: entry.note,
    occurs: occurring.has(index),
  }))
  for (const { drift, occurs } of drifts) {
    if (!occurs) failures.push(`${drift}: listed drift no longer occurs; remove it from knownDrift`)
  }
  return { failures, drifts }
}
