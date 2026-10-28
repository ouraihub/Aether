interface McpConfigInput {
  serverName: string
  url: string
  token: string
}

export function buildMcpJsonConfig({ serverName, url, token }: McpConfigInput): string {
  return JSON.stringify(
    {
      mcpServers: {
        [serverName]: { type: 'http', url, headers: { Authorization: `Bearer ${token}` } },
      },
    },
    null,
    2,
  )
}

function posixQuote(value: string): string {
  return `'${value.replaceAll("'", "'\"'\"'")}'`
}

/** POSIX shell only: every dynamic value is exactly one quoted argument. */
export function buildClaudePosixCommand({ serverName, url, token }: McpConfigInput): string {
  return `claude mcp add --transport http ${posixQuote(serverName)} ${posixQuote(url)} --header ${posixQuote(`Authorization: Bearer ${token}`)}`
}
