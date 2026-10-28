import type {
  Approval,
  ContextUsage,
  Cost,
  EndStatus,
  ImagePart,
  KitError,
  ModelPricing,
  PermissionMode,
  Protocol,
  ReasoningConfig,
  ReasoningEffort,
  TokenUsage,
  ToolEffect,
} from '@ouraihub/aether-contracts'

/** Resolved credential-bearing snapshot. Never log or serialize into public DTOs. */
export interface Connection {
  endpointId: string | null
  modelId: string
  protocol: Protocol
  baseUrl: string
  keys: readonly string[]
  headers: Readonly<Record<string, string>>
  supportsTools: boolean | null
  canVision: boolean
  contextWindow: number | null
  maxOutput: number | null
  reasoning: ReasoningConfig | null
  pricing: ModelPricing | null
  nativeWebSearch?: boolean
  imageBaseUrl?: string
  imageEditBaseUrl?: string
}
export interface ReasoningBlock {
  protocol: Protocol
  endpointId: string | null
  modelId: string | null
  value: unknown
}
export interface ToolCall {
  id: string
  name: string
  args: Record<string, unknown>
}
export type Message =
  | { user: string; images?: readonly ImagePart[] }
  | { assistant: { text: string; tool_calls: ToolCall[]; reasoning?: ReasoningBlock[] } }
  | { tool: { call_id: string; text: string } }
export type ToolSource = { kind: 'host' } | { kind: 'mcp'; server: string }
export interface ToolSpec {
  name: string
  description: string
  parameters: Record<string, unknown>
  approval: Approval
  effect: ToolEffect
  source: ToolSource
}
export interface AuthorizeContext {
  mode: PermissionMode
  purpose: string
  runId: string
  effect: ToolEffect
  source: ToolSource
  proposed: Approval
}
export type ToolExecution =
  | { kind: 'ok'; text: string; payload?: unknown }
  | { kind: 'error'; code: string; message: string; payload?: unknown }
  | { kind: 'unknown'; reason: string }
export type SystemPart =
  | { kind: 'host'; text: string }
  | { kind: 'skill'; id: string; revision: number; name: string; body: string }
  | { kind: 'context'; source: string; text: string }
export interface Budget {
  maxOutput: number
  contextWindow: number
  maxRounds: number
  toolOutputChars: number
  skillsChars: number
  contextChars: number
}
export const DEFAULT_BUDGET: Readonly<Budget> = Object.freeze({
  maxOutput: 4096,
  contextWindow: 32000,
  maxRounds: 5,
  toolOutputChars: 8000,
  skillsChars: 48000,
  contextChars: 24000,
})
export interface RunInput {
  runId: string
  purpose: string
  permissionMode: PermissionMode
  connection: Connection
  system: readonly SystemPart[]
  history: readonly Message[]
  input: string
  images: readonly ImagePart[]
  tools: readonly ToolSpec[]
  budget: Budget
  reasoningEffort: ReasoningEffort
  nativeWebSearch: boolean
}
export interface RunSummary {
  status: EndStatus
  messages: Message[]
  usage: TokenUsage
  cost: Cost | null
  contextUsage: ContextUsage | null
  exhausted: boolean
  error?: KitError
}
export interface HistoryTurn {
  status: string
  input: string
  messages: readonly Message[]
  images?: readonly ImagePart[]
}
