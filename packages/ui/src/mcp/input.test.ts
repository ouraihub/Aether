import { describe, expect, it } from 'vitest'
import type { ServerDto } from '../ports'
import { mapMcpInput, type McpInput } from './input'

const draft = (over: Partial<McpInput> = {}): McpInput => ({
  name: ' Example ',
  slug: ' example ',
  kind: 'http',
  url: ' https://example.com/mcp ',
  command: '',
  args: '',
  cwd: '',
  replaceSecret: true,
  headers: '',
  query: '',
  env: '',
  ...over,
})
const current: ServerDto = {
  id: 'old',
  name: 'Example',
  slug: 'example',
  transport: { kind: 'http', url: 'https://example.com/mcp' },
  enabled: true,
  trusted: true,
  allowPrivate: true,
  secretKeys: ['Authorization'],
  tools: [],
}

function mapped(input: McpInput, server: ServerDto | null = null) {
  const result = mapMcpInput(server, input)
  if (!result.ok) throw new Error(result.error)
  return result.value
}

describe('MCP input mapping', () => {
  it(
    'maps HTTP secrets at only their first separator and ignores blank lines',
    () => {
    expect(
      mapped(
        draft({
          headers: '\nAuthorization: Bearer a:b\n X-Test : x:y ',
          query: '\nak=a=b\n',
          env: 'ignored=env',
        }),
      ),
    ).toEqual({
      name: 'Example',
      slug: 'example',
      transport: { kind: 'http', url: 'https://example.com/mcp' },
      enabled: false,
      trusted: false,
      allowPrivate: false,
      secret: {
        headers: { Authorization: 'Bearer a:b', 'X-Test': 'x:y' },
        query: { ak: 'a=b' },
      },
    })
  })

  it(
    'preserves nonblank args verbatim, trims command/cwd, and sends only ' +
      'env for stdio',
    () => {
    expect(
      mapped(
        draft({
          kind: 'stdio',
          command: ' npx ',
          cwd: ' /work ',
          args: '  spaced  \n\n \n--literal=$x;`y`\n"quoted"',
          env: 'TOKEN=a=b',
          headers: 'malformed',
          query: 'malformed',
        }),
        current,
      ),
    ).toEqual({
      id: 'old',
      name: 'Example',
      slug: 'example',
      transport: {
        kind: 'stdio',
        command: 'npx',
        args: ['  spaced  ', '--literal=$x;`y`', '"quoted"'],
        cwd: '/work',
      },
      enabled: true,
      trusted: true,
      allowPrivate: false,
      secret: { env: { TOKEN: 'a=b' } },
    })
    expect(
      mapped(draft({ kind: 'stdio', command: ' npx ', cwd: '  ' })).transport,
    ).toEqual({
      kind: 'stdio',
      command: 'npx',
      args: [],
    })
  })

  it(
    'distinguishes preservation, explicit clearing, replacement, and empty ' +
      'new input',
    () => {
    expect(
      mapped(draft({ replaceSecret: false, headers: 'ignored' }), current),
    ).not.toHaveProperty('secret')
    expect(mapped(draft(), current).secret).toEqual({})
    expect(
      mapped(draft({ headers: 'Authorization: replacement' }), current).secret,
    ).toEqual({
      headers: { Authorization: 'replacement' },
    })
    expect(mapped(draft())).not.toHaveProperty('secret')
  })

  it(
    'requires explicit replacement or clearing when switching either transport',
    () => {
    expect(
      mapMcpInput(
        current,
        draft({ kind: 'stdio', command: 'npx', replaceSecret: false }),
      ),
    ).toEqual({
      ok: false,
      error: 'transportChanged',
    })
    const stdio = {
      ...current,
      transport: { kind: 'stdio' as const, command: 'npx', args: [] },
    }
    expect(mapMcpInput(stdio, draft({ replaceSecret: false }))).toEqual({
      ok: false,
      error: 'transportChanged',
    })
    expect(mapped(draft(), stdio).secret).toEqual({})
  })

  it.each([
    { headers: 'missing separator' },
    { headers: ' : empty name' },
    { query: '=empty' },
    { query: 'missing' },
    { kind: 'stdio' as const, env: ' =empty' },
  ])('returns a data error for malformed active secret lines: %o', (over) => {
    expect(mapMcpInput(null, draft(over))).toEqual({
      ok: false,
      error: 'secretLine',
    })
  })

  it('treats special property names as data', () => {
    const secret = mapped(
      draft({ query: '__proto__=literal\nconstructor=value' }),
    ).secret
    expect(Object.keys(secret?.query ?? {})).toEqual([
      '__proto__',
      'constructor',
    ])
    expect(secret?.query?.['__proto__']).toBe('literal')
  })
})
