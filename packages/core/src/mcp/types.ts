// Shared MCP configuration and wire result types.
export type Approval = 'auto' | 'ask' | 'deny'
export interface McpTool {
  readonly server: string
  readonly slug: string
  readonly name: string
  readonly prefixed: string
  readonly description: string
  readonly schema: Readonly<Record<string, unknown>>
  readonly approval: Approval
}
export type McpCallResult =
  | {
      readonly kind: 'completed'
      readonly content: readonly Record<string, unknown>[]
      readonly structuredContent?: Record<string, unknown>
      readonly isError: boolean
    }
  | { readonly kind: 'failed'; readonly code: string; readonly message: string }
  | { readonly kind: 'unknown'; readonly reason: string }
export interface McpCachedTool {
  readonly name: string
  readonly description: string
  readonly schema: Readonly<Record<string, unknown>>
  readonly readOnly: boolean
  readonly approval: Approval
  readonly fetchedAt: number
}
export interface McpServerConfig {
  readonly id: string
  readonly slug: string
  readonly name: string
  readonly url: string
  readonly enabled: boolean
  readonly trusted: boolean
  readonly allowPrivate: boolean
  readonly headers: Readonly<Record<string, string>>
  readonly query: Readonly<Record<string, string>>
  readonly tools: readonly McpCachedTool[]
}
export interface McpConfigStore {
  list(): Promise<readonly McpServerConfig[]>
  get(id: string): Promise<McpServerConfig | null>
  replaceTools(id: string, tools: readonly McpCachedTool[]): Promise<void>
}
