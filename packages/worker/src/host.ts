import type {
  Approval,
  ModelRef,
  PermissionMode,
  RunRequest,
} from '@ouraihub/aether-contracts'
import type {
  AuthorizeContext,
  ToolCall,
  ToolExecution,
  ToolSpec,
} from '@ouraihub/aether-core/types'
import type { WorkerContext } from './context.js'

/** Business authorization remains the host's responsibility for every actor and resource. */
export interface WorkerHost {
  selectModel(request: RunRequest, context: WorkerContext): Promise<ModelRef>
  systemPrompt(
    request: RunRequest,
    context: WorkerContext,
  ): Promise<string | null>
  context(
    request: RunRequest,
    context: WorkerContext,
  ): Promise<{ source: string; text: string } | null>
  tools(
    request: RunRequest,
    runId: string,
    context: WorkerContext,
  ): Promise<readonly ToolSpec[]>
  permissionModes(
    request: RunRequest,
    context: WorkerContext,
  ): readonly PermissionMode[]
  authorize(
    call: ToolCall,
    authorization: AuthorizeContext,
    context: WorkerContext,
  ): Approval
  callTool(
    call: ToolCall,
    signal: AbortSignal,
    context: WorkerContext,
  ): Promise<ToolExecution>
  runFinished(runId: string, context: WorkerContext): Promise<void>
  mcpTools?(request: RunRequest, context: WorkerContext): boolean
  paintingModel?(
    request: RunRequest,
    context: WorkerContext,
  ): Promise<ModelRef | null>
}
