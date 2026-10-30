// The tool-side I/O of a run: the bounded approval wait and the bounded execution. What to do
// with their answers is decided in `machine.ts`.
import type { Approver, ToolExecutor } from '../ports.js'
import type { ToolCall, ToolExecution } from '../types.js'
import { KitFailure } from '../error.js'
import { boundedWait } from './model.js'

/** The user's answer; `false` after the timeout, `null` when the run was cancelled. */
export async function waitApproval(
  approver: Approver,
  runId: string,
  call: ToolCall,
  signal: AbortSignal,
  timeoutMs: number,
): Promise<boolean | null> {
  const controller = new AbortController()
  try {
    return await boundedWait(
      approver.wait(runId, call, controller.signal),
      signal,
      timeoutMs,
      () => controller.abort(),
      'approval',
    )
  } catch (error) {
    if (signal.aborted) return null
    if (error instanceof KitFailure && error.code === 'timeout') return false
    throw error
  } finally {
    controller.abort()
  }
}

export async function executeCall(
  executor: ToolExecutor,
  call: ToolCall,
  signal: AbortSignal,
  timeoutMs: number,
): Promise<ToolExecution> {
  const controller = new AbortController()
  const pending = Promise.resolve().then(() => executor.execute(call, controller.signal))
  try {
    return await boundedWait(pending, signal, timeoutMs, () => controller.abort(), 'tool')
  } catch {
    controller.abort()
    // A dispatched tool may still confirm its result after cancellation; otherwise preserve uncertainty.
    const settled = pending.catch((): ToolExecution => ({
      kind: 'unknown',
      reason: 'Executor failed after dispatch',
    }))
    const deadline = Promise.withResolvers<ToolExecution>()
    const timer = setTimeout(
      () =>
        deadline.resolve({
          kind: 'unknown',
          reason: 'Execution result could not be confirmed after cancellation or timeout',
        }),
      1000,
    )
    try {
      return await Promise.race([settled, deadline.promise])
    } finally {
      clearTimeout(timer)
    }
  } finally {
    controller.abort()
  }
}
