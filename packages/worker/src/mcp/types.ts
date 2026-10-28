import type { OutboundHttp } from '@ouraihub/aether-core'
import type { Actor } from '../context.js'
import type { CredentialPort } from '../credentials.js'
import type { DocumentStore } from '../storage.js'

export interface McpScope {
  readonly id: string
  readonly label: string
  readonly destructive: boolean
}
export interface ExposedMcpTool {
  readonly name: string
  readonly description: string
  readonly inputSchema: Readonly<Record<string, unknown>>
  readonly scope: string
  readonly readOnly: boolean
  readonly destructive: boolean
  readonly idempotent: boolean
  readonly maxRequestBytes?: number
  readonly validate: (args: Readonly<Record<string, unknown>>) => boolean
  readonly handler: (args: Readonly<Record<string, unknown>>, context: McpCallContext) => Promise<unknown>
}
export interface McpCallContext {
  readonly actor: Actor
  readonly tokenId: string
  readonly scopes: readonly string[]
  readonly signal: AbortSignal
}
export interface McpServiceOptions {
  readonly applicationId: string
  readonly origin: string
  readonly store: DocumentStore
  readonly credentials: CredentialPort
  readonly outbound: OutboundHttp
  readonly scopes: readonly McpScope[]
  readonly tools: readonly ExposedMcpTool[]
  readonly resolveIdentity: (request: Request) => Promise<Actor | null>
  /** Must check the actor may grant every requested scope, not just that login exists. */
  readonly authorizeConsent: (actor: Actor, scopes: readonly string[], request: Request) => Promise<boolean>
  /** Must check resource/business access immediately before external tool dispatch. */
  readonly authorizeTool: (
    actor: Actor,
    tool: ExposedMcpTool,
    args: Readonly<Record<string, unknown>>,
    signal: AbortSignal,
  ) => Promise<boolean>
  readonly mcpOrigins?: readonly string[]
  readonly now?: () => number
}
export interface McpSettings {
  readonly enabled: boolean
  readonly enabledScopes: readonly string[]
}
export interface GrantRecord {
  readonly id: string
  readonly applicationId: string
  readonly tenantId: string | null
  readonly subjectId: string
  readonly clientId: string
  readonly redirectUri: string
  readonly scopes: readonly string[]
  readonly createdAt: number
  readonly expiresAt: number | null
  readonly revokedAt: number | null
}
export interface ApiKeyRecord {
  readonly id: string
  readonly name: string
  readonly scopes: readonly string[]
  readonly prefix: string
  readonly createdAt: number
  readonly expiresAt: number | null
  readonly revokedAt: number | null
}
