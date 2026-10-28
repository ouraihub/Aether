import type { Approval, RunEvent } from '@ouraihub/aether-contracts'
import type { AuthorizeContext, ToolCall, ToolExecution } from './types.js'

/** Implementations enforce address policy, reject redirects, and propagate signal. */
export interface OutboundHttp {
  fetch(url: string, init: RequestInit): Promise<Response>
}
export interface ToolExecutor {
  authorize(call: ToolCall, context: AuthorizeContext): Approval
  execute(call: ToolCall, signal: AbortSignal): Promise<ToolExecution>
}
export interface Approver {
  wait(runId: string, call: ToolCall, signal: AbortSignal): Promise<boolean>
}
/** Validators must be compiled before deployment; no runtime code generation. */
export type ArgumentsValidator = (
  schema: Readonly<Record<string, unknown>>,
  args: Readonly<Record<string, unknown>>,
) => boolean
export type EventSink = (event: RunEvent) => void
