// Port interfaces and KitClientError (docs/16 §2, docs/04 §3.2). Pure TS: no Tauri imports.
import type { Conversation } from './generated/Conversation'
import type { ConversationSummary } from './generated/ConversationSummary'
import type { IngestBatch } from './generated/IngestBatch'
import type { IngestReceipt } from './generated/IngestReceipt'
import type { IssuedToken } from './generated/IssuedToken'
import type { KitError } from './generated/KitError'
import type { KitErrorCode } from './generated/KitErrorCode'
import type { Details } from './generated/Details'
import type { LogEntry } from './generated/LogEntry'
import type { LogLevel } from './generated/LogLevel'
import type { LogStatus } from './generated/LogStatus'
import type { ManualSave } from './generated/ManualSave'
import type { Approval } from './generated/Approval'
import type { ModelFilter } from './generated/ModelFilter'
import type { ModelOption } from './generated/ModelOption'
import type { ProbeResult } from './generated/ProbeResult'
import type { Preview } from './generated/Preview'
import type { ProbeTarget } from './generated/ProbeTarget'
import type { Provider } from './generated/Provider'
import type { ProviderSave } from './generated/ProviderSave'
import type { RunAccepted } from './generated/RunAccepted'
import type { RunEvent } from './generated/RunEvent'
import type { RunRequest } from './generated/RunRequest'
import type { ServerDto } from './generated/ServerDto'
import type { ServerSave } from './generated/ServerSave'
import type { SkillDto } from './generated/SkillDto'
import type { SkillMeta } from './generated/SkillMeta'
import type { Source } from './generated/Source'
import type { ScopeState } from './generated/ScopeState'
import type { Status } from './generated/Status'
import type { TokenRecord } from './generated/TokenRecord'
import type { ToolDto } from './generated/ToolDto'
import type { SearchPort } from './search'

export interface CallOptions {
  signal?: AbortSignal
}

export interface LogsPort {
  tail(limit: number): Promise<LogEntry[]>
  ingest(batch: IngestBatch): Promise<IngestReceipt>
  openDir(): Promise<void>
  getLevel(): Promise<LogStatus>
  /** Takes effect immediately and is persisted to `level.json`. */
  setLevel(level: LogLevel): Promise<LogStatus>
}

export interface ProvidersPort {
  list(): Promise<Provider[]>
  save(provider: ProviderSave): Promise<Provider[]>
  remove(id: string): Promise<Provider[]>
  modelOptions(filter?: ModelFilter): Promise<ModelOption[]>
  probe(target: ProbeTarget, options?: CallOptions): Promise<ProbeResult>
}

export interface ChatHistoryPort {
  list(scope?: string): Promise<ConversationSummary[]>
  get(id: string): Promise<Conversation>
  remove(id: string): Promise<void>
  rename(id: string, title: string): Promise<ConversationSummary>
  editTurn(conversationId: string, turnId: string, text: string, expectedText: string): Promise<Conversation>
  removeTurn(conversationId: string, turnId: string): Promise<Conversation>
  fork(conversationId: string, turnId: string): Promise<Conversation>
  selectTurn(conversationId: string, turnId: string): Promise<Conversation>
}

export interface ChatPort {
  /** Resolves once the run is accepted; the outcome arrives as the single `end` event. */
  run(request: RunRequest, onEvent: (event: RunEvent) => void, options?: CallOptions): Promise<RunAccepted>
  approve(runId: string, callId: string, allow: boolean): Promise<void>
  /** Absent when the host passed no conversation store. */
  history?: ChatHistoryPort
}

export interface SkillsPort {
  list(): Promise<SkillMeta[]>
  get(id: string): Promise<SkillDto>
  /** Reads and validates a package; nothing is stored. */
  preview(source: Source): Promise<Preview>
  /** `confirmedDigest` is the previewed digest; a changed package rejects with `conflict{content_changed}`. */
  install(source: Source, confirmedDigest: string, replace: boolean): Promise<SkillMeta[]>
  save(input: ManualSave): Promise<SkillMeta[]>
  duplicate(id: string, newName: string): Promise<SkillMeta[]>
  reorder(ids: readonly string[]): Promise<SkillMeta[]>
  remove(id: string): Promise<SkillMeta[]>
}

export interface McpClientPort {
  list(): Promise<ServerDto[]>
  save(server: ServerSave): Promise<ServerDto[]>
  remove(id: string): Promise<ServerDto[]>
  refresh(serverId: string): Promise<ToolDto[]>
  setToolApproval(serverId: string, tool: string, approval: Approval): Promise<void>
}

export interface McpServerPort {
  status(): Promise<Status>
  tokens(): Promise<TokenRecord[]>
  start(): Promise<Status>
  stop(): Promise<Status>
  setPort(port: number): Promise<Status>
  issueToken(name: string, scopes: readonly string[]): Promise<IssuedToken>
  revokeToken(id: string): Promise<void>
  scopes(): Promise<ScopeState[]>
  setScopes(ids: readonly string[]): Promise<ScopeState[]>
}

/** Ports the host declared; components skip the area of a missing port (docs/16 §2). */
export interface KitPorts {
  /** Absent on hosts without a local log file, such as a browser. */
  logs?: LogsPort
  providers?: ProvidersPort
  chat?: ChatPort
  search?: SearchPort
  skills?: SkillsPort
  mcpClient?: McpClientPort
  mcpServer?: McpServerPort
}

/** The only error type thrown across a port (docs/04 §3.2). */
export class KitClientError extends Error {
  readonly code: KitErrorCode
  readonly details: Details

  constructor(error: KitError) {
    super(error.message)
    this.name = 'KitClientError'
    this.code = error.code
    this.details = error.details ?? {}
  }
}

export function isKitClientError(error: unknown): error is KitClientError {
  return error instanceof KitClientError
}
