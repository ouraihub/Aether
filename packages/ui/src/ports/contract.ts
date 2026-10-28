// Port interfaces and KitClientError (docs/16 §2, docs/04 §3.2). Pure TS: no Tauri imports.
import type {
  Conversation,
  ConversationSummary,
  IngestBatch,
  IngestReceipt,
  IssuedToken,
  KitError,
  KitErrorCode,
  Details,
  EndpointKeySave,
  LogEntry,
  LogLevel,
  LogStatus,
  ManualSave,
  Approval,
  ModelFilter,
  ModelOption,
  ProbeResult,
  Preview,
  ProbeTarget,
  Provider,
  ProviderSave,
  Protocol,
  RunAccepted,
  RunEvent,
  RunRequest,
  RunSnapshot,
  ServerDto,
  ServerSave,
  SkillDto,
  SkillMeta,
  Source,
  ScopeState,
  Status,
  TokenRecord,
  ToolDto,
} from '@ouraihub/aether-contracts'
import type { SearchPort } from './search'
import type { SkillSourcesPort } from './skill-sources'

export interface CallOptions {
  signal?: AbortSignal
}

export interface LogsPort {
  tail(limit: number): Promise<LogEntry[]>
  ingest(batch: IngestBatch): Promise<IngestReceipt>
  getLevel(): Promise<LogStatus>
  /** Takes effect immediately and is persisted to `level.json`. */
  setLevel(level: LogLevel): Promise<LogStatus>
}

export interface DesktopActions {
  openLogsDirectory(): Promise<void>
}

/** Host-supplied provider template, seeded once into the store disabled and keyless (docs/39). */
export interface ProviderPreset {
  id: string
  name: string
  baseUrl: string
  protocol: Protocol
  /** "Get a key" link, e.g. an invite link. */
  apiKeyUrl?: string
}

export interface ProvidersPort {
  /** Browser asks for the current password; desktop uses its local credential boundary. */
  readonly revealNeedsPassword: boolean
  /** Host stores `keyStrategy` and key set-aside state (web); `false` = rotation only, fields ignored. */
  readonly keyStrategies: boolean
  list(): Promise<Provider[]>
  setEnabled(id: string, enabled: boolean): Promise<Provider[]>
  save(provider: ProviderSave): Promise<Provider[]>
  remove(id: string): Promise<Provider[]>
  modelOptions(filter?: ModelFilter): Promise<ModelOption[]>
  probe(target: ProbeTarget, options?: CallOptions): Promise<ProbeResult>
  saveKey(input: EndpointKeySave): Promise<Provider[]>
  removeKey(endpointId: string, keyId: string): Promise<Provider[]>
  revealKey(keyId: string, currentPassword?: string | null): Promise<string>
}

export interface ConversationClient {
  list(scope?: string): Promise<ConversationSummary[]>
  get(id: string): Promise<Conversation>
  remove(id: string): Promise<void>
  rename(id: string, title: string): Promise<ConversationSummary>
  editTurn(conversationId: string, turnId: string, text: string, expectedText: string): Promise<Conversation>
  removeTurn(conversationId: string, turnId: string): Promise<Conversation>
  fork(conversationId: string, turnId: string): Promise<Conversation>
  selectTurn(conversationId: string, turnId: string): Promise<Conversation>
}

export interface RunClient {
  /** Resolves once accepted; events may arrive before acceptance. */
  start(request: RunRequest, onEvent: (event: RunEvent) => void, options?: CallOptions): Promise<RunAccepted>
  cancel(runId: string): Promise<void>
  approve(runId: string, callId: string, allow: boolean): Promise<void>
  getSnapshot(runId: string): Promise<RunSnapshot>
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
  logs?: LogsPort
  desktop?: DesktopActions
  providers?: ProvidersPort
  chat?: RunClient
  conversations?: ConversationClient
  search?: SearchPort
  skills?: SkillsPort
  skillSources?: SkillSourcesPort
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
