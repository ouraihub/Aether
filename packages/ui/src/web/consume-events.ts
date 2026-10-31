import { validateRunEvent } from '@ouraihub/aether-contracts'
import type { RunEvent } from '@ouraihub/aether-contracts'
import { KitClientError } from '../ports/contract'
const invalidResponse = () =>
  new KitClientError({
    code: 'upstream',
    message: 'Invalid server response',
    details: { reason: 'invalid_response' },
  })

export async function consumeEvents(
  body: ReadableStream<Uint8Array>,
  emit: (event: RunEvent) => void,
  ended: () => boolean,
): Promise<void> {
  const reader = body.getReader()
  const decoder = new TextDecoder('utf-8', { fatal: true })
  let buffer = ''
  let data: string[] = []
  let eventName = ''
  let size = 0
  const line = (value: string) => {
    if (value === '') {
      if (data.length > 0) {
        if (eventName && eventName !== 'run_event') throw invalidResponse()
        let parsed: unknown
        try {
          parsed = JSON.parse(data.join('\n'))
        } catch {
          throw invalidResponse()
        }
        if (!validateRunEvent(parsed)) throw invalidResponse()
        emit(parsed)
      }
      data = []
      eventName = ''
      size = 0
      return
    }
    size += value.length
    if (size > 1024 * 1024) throw invalidResponse()
    if (value.startsWith(':')) return
    const separator = value.indexOf(':')
    const field = separator < 0 ? value : value.slice(0, separator)
    let content = separator < 0 ? '' : value.slice(separator + 1)
    if (content.startsWith(' ')) content = content.slice(1)
    if (field === 'data') data.push(content)
    else if (field === 'event') eventName = content
  }
  try {
    for (;;) {
      const chunk = await reader.read()
      buffer += decoder.decode(chunk.value, { stream: !chunk.done })
      let at = 0
      while (at < buffer.length) {
        const found = buffer.slice(at).search(/[\r\n]/)
        if (found < 0) break
        const end = at + found
        if (!chunk.done && buffer[end] === '\r' && end === buffer.length - 1)
          break
        line(buffer.slice(at, end))
        at += end - at +
          (buffer[end] === '\r' && buffer[end + 1] === '\n' ? 2 : 1)
        if (ended()) return
      }
      buffer = buffer.slice(at)
      if (buffer.length + size > 1024 * 1024) throw invalidResponse()
      if (chunk.done) return
    }
  } finally {
    try {
      await reader.cancel()
    } finally {
      reader.releaseLock()
    }
  }
}
