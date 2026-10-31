import { KitFailure } from './error.js'

export interface SseEvent {
  event: string | null
  data: string
}
/** Limits count encoded bytes, including unterminated lines and multiline data. */
export class SseParser {
  private readonly decoder = new TextDecoder('utf-8', {
    fatal: true,
    ignoreBOM: false,
  })
  private line = ''
  private event: string | null = null
  private data: string[] = []
  private lineBytes = 0
  private eventBytes = 0
  private hasData = false
  private readonly maxLineBytes: number
  private readonly maxEventBytes: number
  constructor(maxLineBytes = 1_048_576, maxEventBytes = 2_097_152) {
    this.maxLineBytes = maxLineBytes
    this.maxEventBytes = maxEventBytes
  }
  private afterCr = false
  feed(chunk: Uint8Array): SseEvent[] {
    let text: string
    try {
      text = this.decoder.decode(chunk, { stream: true })
    } catch {
      throw new KitFailure('upstream', 'Invalid UTF-8 stream')
    }
    return this.consume(text)
  }
  finish(): SseEvent[] {
    let text: string
    try {
      text = this.decoder.decode()
    } catch {
      throw new KitFailure('upstream', 'Incomplete UTF-8 stream')
    }
    const events = this.consume(text)
    if (this.line) this.flushLine(events)
    this.flushLine(events)
    return events
  }
  private consume(text: string): SseEvent[] {
    const events: SseEvent[] = []
    for (const char of text) {
      if (this.afterCr) {
        this.afterCr = false
        if (char === '\n') continue
      }
      if (char === '\r' || char === '\n') {
        this.flushLine(events)
        this.afterCr = char === '\r'
        continue
      }
      const firstUnit = char.charCodeAt(0)
      this.lineBytes +=
        firstUnit <= 0x7f
          ? 1
          : firstUnit <= 0x7ff
            ? 2
            : char.length === 2
              ? 4
              : 3
      if (this.lineBytes > this.maxLineBytes)
        throw new KitFailure('upstream', 'SSE line limit exceeded', {
          reason: 'sse_limit',
        })
      this.line += char
    }
    return events
  }
  private flushLine(events: SseEvent[]): void {
    const line = this.line
    const size = this.lineBytes
    this.line = ''
    this.lineBytes = 0
    if (!line) {
      if (this.hasData)
        events.push({ event: this.event, data: this.data.join('\n') })
      this.data = []
      this.event = null
      this.eventBytes = 0
      this.hasData = false
      return
    }
    const colon = line.indexOf(':')
    const field = colon < 0 ? line : line.slice(0, colon)
    const raw = colon < 0 ? '' : line.slice(colon + 1)
    const value = raw.startsWith(' ') ? raw.slice(1) : raw
    if (field !== 'event' && field !== 'data') return
    this.eventBytes += size + 1
    if (this.eventBytes > this.maxEventBytes)
      throw new KitFailure('upstream', 'SSE event limit exceeded', {
        reason: 'sse_limit',
      })
    if (field === 'event') this.event = value || null
    else {
      this.hasData = true
      this.data.push(value)
    }
  }
}
