export type LogEvent =
  | 'request.finished'
  | 'auth.rejected'
  | 'session.revoked'
  | 'mcp.grant.created'
  | 'mcp.grant.revoked'
  | 'mcp.api_key.created'
  | 'mcp.api_key.revoked'
  | 'tool.denied'
  | 'run.finished'
  | 'run.unknown'
  | 'credential.decrypt_failed'
export type LogValue = string | number | boolean
export interface Logger {
  log(event: LogEvent, fields: Readonly<Record<string, unknown>>): void
}
export interface LogRecord {
  readonly event: LogEvent
  readonly fields: Readonly<Record<string, LogValue>>
}
const EVENTS: Readonly<Record<LogEvent, true>> = {
  'request.finished': true,
  'auth.rejected': true,
  'session.revoked': true,
  'mcp.grant.created': true,
  'mcp.grant.revoked': true,
  'mcp.api_key.created': true,
  'mcp.api_key.revoked': true,
  'tool.denied': true,
  'run.finished': true,
  'run.unknown': true,
  'credential.decrypt_failed': true,
}
const IDENTIFIERS: Readonly<Record<string, true>> = {
  requestId: true,
  runId: true,
  applicationId: true,
  actorHash: true,
  providerId: true,
  endpointId: true,
  modelId: true,
  toolId: true,
  errorCode: true,
}
const COUNTERS: Readonly<Record<string, true>> = {
  status: true,
  durationMs: true,
  count: true,
  eventCount: true,
  revision: true,
  rounds: true,
}
/** Pass only internal opaque identifiers, never prompt/header objects. Unknown fields are dropped.
 * Route values must be registered templates, not request URLs or user-supplied paths. */
export function createLogger(options: {
  readonly sink: (record: LogRecord) => void
  readonly routes: readonly string[]
}): Logger {
  const routes = new Set(options.routes)
  for (const route of routes)
    if (!/^\/[A-Za-z0-9_/:.{}-]*$/u.test(route) || route.length > 256) throw new Error('Invalid log route template')
  return Object.freeze({
    log(event: LogEvent, fields: Readonly<Record<string, unknown>>): void {
      if (!Object.hasOwn(EVENTS, event)) throw new Error('Invalid log event')
      const safe: Record<string, LogValue> = {}
      for (const [key, value] of Object.entries(fields)) {
        if (Object.hasOwn(IDENTIFIERS, key) && typeof value === 'string' && /^[A-Za-z0-9_.:-]{1,128}$/u.test(value))
          safe[key] = value
        else if (Object.hasOwn(COUNTERS, key) && typeof value === 'number' && Number.isSafeInteger(value) && value >= 0)
          safe[key] = value
        else if (key === 'usageAvailable' && typeof value === 'boolean') safe[key] = value
        else if (key === 'route' && typeof value === 'string' && routes.has(value)) safe[key] = value
        else if (
          key === 'state' &&
          (value === 'accepted' ||
            value === 'running' ||
            value === 'completed' ||
            value === 'failed' ||
            value === 'cancelled' ||
            value === 'unknown')
        )
          safe[key] = value
      }
      options.sink(Object.freeze({ event, fields: Object.freeze(safe) }))
    },
  })
}
