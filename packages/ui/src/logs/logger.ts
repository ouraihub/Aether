// createLogger (docs/16 §4.5): same whitelist as the backend (docs/10 §10),
// batched into LogsPort.ingest.
// ponytail: secret detection stays in the backend (it rejects and counts);
// mirror mask.rs here if a secret-looking value must never leave the page.
import type { KitLogger, LogFields } from '../context'
import type { IngestEntry, IngestLevel, LogsPort } from '../ports'

const MAX_QUEUE = 200
const BATCH = 20
const MAX_BATCH = 50
const DELAY_MS = 250
const MAX_FIELDS = 16
const MAX_STRING = 200
const EVENT = /^[a-z][a-z0-9_]*(\.[a-z0-9_]+){1,5}$/
const KEY = /^[a-z][a-z0-9_]*(\.[a-z0-9_]+){0,5}$/

export interface BatchingLogger extends KitLogger {
  /** Sends everything queued now (also done on `pagehide`). */
  flush(): Promise<void>
}

function isValid(event: string, fields: LogFields | undefined): boolean {
  if (!EVENT.test(event)) return false
  const entries = Object.entries(fields ?? {})
  return (
    entries.length <= MAX_FIELDS &&
    entries.every(([key, value]) => {
      if (!KEY.test(key)) return false
      if (typeof value === 'string') return [...value].length <= MAX_STRING
      return typeof value === 'boolean' || Number.isFinite(value)
    })
  )
}

export function createLogger(port: LogsPort): BatchingLogger {
  const queue: IngestEntry[] = []
  let dropped = 0
  let timer: number | undefined
  let inFlight: Promise<void> | null = null

  // One request at a time; entries queue meanwhile, bounded by MAX_QUEUE.
  async function drain(): Promise<void> {
    while (queue.length > 0 || dropped > 0) {
      const entries = queue.splice(0, MAX_BATCH)
      const droppedCount = dropped
      dropped = 0
      try {
        await port.ingest({ entries, droppedCount })
      } catch {
        dropped += droppedCount + entries.length
        return
      }
    }
  }

  function schedule(): void {
    timer ??= window.setTimeout(() => void flush(), DELAY_MS)
  }

  function flush(): Promise<void> {
    window.clearTimeout(timer)
    timer = undefined
    inFlight ??= drain().finally(() => {
      inFlight = null
      if (queue.length > 0) schedule()
    })
    return inFlight
  }

  function push(level: IngestLevel, event: string, fields?: LogFields): void {
    if (!isValid(event, fields)) {
      dropped++
      return
    }
    if (queue.length >= MAX_QUEUE) {
      // Full: drop the oldest debug/info first; a new debug/info entry is
      // dropped itself.
      dropped++
      const low = queue.findIndex(
        (entry) => entry.level === 'debug' || entry.level === 'info',
      )
      if (low >= 0) queue.splice(low, 1)
      else if (level === 'debug' || level === 'info') return
      else queue.shift()
    }
    queue.push(fields ? { level, event, fields } : { level, event })
    if (queue.length >= BATCH) void flush()
    else schedule()
  }

  window.addEventListener('pagehide', () => void flush())

  return {
    debug: (event, fields) => push('debug', event, fields),
    info: (event, fields) => push('info', event, fields),
    warn: (event, fields) => push('warn', event, fields),
    error: (event, fields) => push('error', event, fields),
    flush,
  }
}
