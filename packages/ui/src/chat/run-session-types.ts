import type {
  ContextRef,
  ContextUsage,
  Cost,
  ImageRef,
  ModelRef,
  PermissionMode,
  ReasoningEffort,
  SearchHit,
  SkillRef,
  TokenUsage,
  ToolOutcome,
  TurnStatus,
} from '../ports'

export interface ToolItem {
  callId: string
  name: string
  server: string | null
  pending: string | null
  outcome: ToolOutcome | null
  summary: string
}

export interface ChatTurn {
  id: string
  persisted: boolean
  parentId: string | null
  questionId: string
  supersedesId: string | null
  kind: 'reply' | 'translation'
  endpointId: string | null
  modelId: string | null
  input: string
  images: ImageRef[]
  generated: ImageRef[]
  text: string
  reasoningText: string
  reasoningEffort: ReasoningEffort
  usage: TokenUsage | null
  cost: Cost | null
  contextUsage: ContextUsage | null
  status: TurnStatus
  error: string | null
  tools: ToolItem[]
  exhausted: boolean
  search: SearchState
}

export interface SearchState {
  status: 'idle' | 'searching' | 'complete' | 'failed'
  query: string | null
  provider: string | null
  hits: SearchHit[]
  error: string | null
}

export interface RunBase {
  purpose: string
  scope: string | null
  language: string
  model: ModelRef | null
  skills: SkillRef[]
  context: ContextRef | null
  reasoningEffort?: ReasoningEffort
  generateImage?: boolean
  permissionMode?: PermissionMode
}
