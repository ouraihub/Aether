import type {
  McpTransport,
  ServerDto,
  ServerSave,
  ServerSecret,
} from '../ports'

/** Write-only editor input; credentials never come from a ServerDto. */
export interface McpInput {
  name: string
  slug: string
  kind: McpTransport['kind']
  url: string
  command: string
  args: string
  cwd: string
  replaceSecret: boolean
  headers: string
  query: string
  env: string
}

export type McpInputResult =
  | { ok: true; value: ServerSave }
  | { ok: false; error: 'secretLine' | 'transportChanged' }

/** Split only at the first separator; values may contain separators. */
function pairs(text: string, separator: string): Record<string, string> | null {
  const entries: [string, string][] = []
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    const at = line.indexOf(separator)
    const name = line.slice(0, at).trim()
    if (at <= 0 || !name) return null
    entries.push([name, line.slice(at + 1).trim()])
  }
  return Object.fromEntries(entries)
}

export function mapMcpInput(
  current: ServerDto | null,
  input: McpInput,
): McpInputResult {
  if (
    current &&
    current.transport.kind !== input.kind &&
    !input.replaceSecret
  ) {
    return { ok: false, error: 'transportChanged' }
  }
  const transport: McpTransport =
    input.kind === 'http'
      ? { kind: 'http', url: input.url.trim() }
      : {
          kind: 'stdio',
          command: input.command.trim(),
          args: input.args.split('\n').filter((arg) => arg.trim() !== ''),
          ...(input.cwd.trim() ? { cwd: input.cwd.trim() } : {}),
        }
  const value: ServerSave = {
    ...(current ? { id: current.id } : {}),
    name: input.name.trim(),
    slug: input.slug.trim(),
    transport,
    enabled: current?.enabled ?? false,
    trusted: current?.trusted ?? false,
    allowPrivate:
      transport.kind === 'http' ? (current?.allowPrivate ?? false) : false,
  }
  if (input.replaceSecret) {
    const secret: ServerSecret = {}
    if (input.kind === 'http') {
      const headers = pairs(input.headers, ':')
      const query = pairs(input.query, '=')
      if (!headers || !query) return { ok: false, error: 'secretLine' }
      if (Object.keys(headers).length) secret.headers = headers
      if (Object.keys(query).length) secret.query = query
    } else {
      const env = pairs(input.env, '=')
      if (!env) return { ok: false, error: 'secretLine' }
      if (Object.keys(env).length) secret.env = env
    }
    // Omitted preserves; explicit {} clears. New servers need no empty secret.
    if (current || Object.keys(secret).length) value.secret = secret
  }
  return { ok: true, value }
}
