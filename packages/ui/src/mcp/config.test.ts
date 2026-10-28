import { describe, expect, it } from 'vitest'
import { buildClaudePosixCommand, buildMcpJsonConfig } from './config'

describe('MCP configuration generation', () => {
  it('preserves the existing JSON structure and encodes dynamic keys and values', () => {
    const input = { serverName: '__proto__"\n', url: 'https://example.com/"\\\n', token: 'a"\\\n$`;' }
    const text = buildMcpJsonConfig(input)
    expect(JSON.parse(text)).toEqual({
      mcpServers: {
        [input.serverName]: { type: 'http', url: input.url, headers: { Authorization: `Bearer ${input.token}` } },
      },
    })
    expect(text).toBe(JSON.stringify(JSON.parse(text), null, 2))
  })

  it('quotes every dynamic POSIX argument including the entire authorization header', () => {
    expect(
      buildClaudePosixCommand({ serverName: 'aether', url: 'http://127.0.0.1:3849/mcp', token: 'example-token' }),
    ).toBe(
      "claude mcp add --transport http 'aether' 'http://127.0.0.1:3849/mcp' --header 'Authorization: Bearer example-token'",
    )
  })

  it('keeps whitespace, quotes, newlines, dollar signs, backticks, and semicolons as argument data', () => {
    expect(
      buildClaudePosixCommand({
        serverName: "a b'c",
        url: "https://example.com/a\n'$HOME`id`;",
        token: 'x\'y\n$z`id`;"',
      }),
    ).toBe(
      "claude mcp add --transport http 'a b'\"'\"'c' 'https://example.com/a\n'\"'\"'$HOME`id`;' --header 'Authorization: Bearer x'\"'\"'y\n$z`id`;\"'",
    )
  })
})
