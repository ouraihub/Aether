import type { ImageRef, ModelRef, PermissionMode, ReasoningEffort, SkillRef } from '../ports'

/** Host inputs for the chat container, independent of its private view composition. */
export interface ChatPanelProps {
  purpose?: string
  model?: ModelRef | null
  scope?: string
  /** Host context can be removed for the next message without changing conversation ownership. */
  context?: { kind: string; id: string; label: string } | null
  skills?: SkillRef[]
  onSkillsChange?: (refs: SkillRef[]) => void
  skillsBudget?: number
  onInsert?: (markdown: string) => void
  onSaveNote?: (markdown: string, images: ImageRef[]) => Promise<void> | void
  onExportMarkdown?: (markdown: string) => Promise<void> | void
  onOpenSettings?: () => void
  /** A failed restore stays visible until retried or replaced with a new conversation. */
  conversationId?: string | null
  onConversationChange?: (id: string | null) => void
  onClose?: () => void
  /** A host painting model gates image generation. */
  paintingModel?: boolean
  /** Ordered pinned tool ids; omitted means the default toolbar. */
  toolbar?: string[]
  onToolbarChange?: (ids: string[]) => void
  /** Saved preference mapped to the selected model's nearest supported effort. */
  reasoningEffort?: ReasoningEffort
  onReasoningEffortChange?: (effort: ReasoningEffort) => void
  permissionModes?: PermissionMode[]
  /** Full access stays private and is consumed once, never saved as a preference. */
  permissionMode?: PermissionMode
}
