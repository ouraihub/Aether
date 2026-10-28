import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { SseParser } from '../src/sse.js'
import { Decoder } from '../src/protocol/decoder.js'
import { emptyUsage } from '../src/pricing.js'
import { at } from '../src/guard.js'

const root = new URL('../../ui/fixtures/sse/', import.meta.url)
for (const protocol of ['anthropic-messages', 'openai-chat', 'openai-responses'] as const) {
  for (const name of ['text', 'chinese', 'truncated', 'upstream-error', 'single-tool', 'multi-tool']) {
    test(`${protocol}/${name}: shared native fixture across UTF-8 boundaries`, async () => {
      const bytes = new Uint8Array(await readFile(new URL(`${protocol}/${name}.sse`, root)))
      const rawExpectations: unknown = JSON.parse(await readFile(new URL('expect.json', root), 'utf8'))
      for (const stride of [1, 7, 131, bytes.length]) {
        const parser = new SseParser(),
          decoder = new Decoder(protocol, 'endpoint', 'model'),
          usage = emptyUsage()
        let text = '',
          error: string | undefined
        for (let index = 0; index < bytes.length; index += stride) {
          for (const event of parser.feed(bytes.subarray(index, index + stride))) {
            const decoded = decoder.event(event.data, usage)
            if (decoded?.text) text += decoded.text
            if (decoded?.error) error = decoded.error
          }
        }
        for (const event of parser.finish()) {
          const decoded = decoder.event(event.data, usage)
          if (decoded?.text) text += decoded.text
          if (decoded?.error) error = decoded.error
        }
        const expected = at(rawExpectations, protocol, name)
        if (expected) {
          assert.equal(text, at(expected, 'text'))
          if (name === 'upstream-error')
            assert.ok(error, 'upstream errors must be surfaced alongside preserved partial text')
          else {
            assert.equal(decoder.done, true)
            assert.equal(usage.inputTokens, at(expected, 'inputTokens'))
            assert.equal(usage.outputTokens, at(expected, 'outputTokens'))
          }
        } else {
          const calls = decoder.toolCalls()
          assert.equal(decoder.done, true)
          assert.equal(calls.length, name === 'single-tool' ? 1 : 2)
          assert.ok(calls.every((call) => call.id && call.name && call.args !== null))
        }
      }
    })
  }
}
