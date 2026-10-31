// Default KitNotify for hosts without their own: svelte-sonner toasts + one
// shadcn AlertDialog (docs/16 §3). Render <KitNotifier notify={...} /> once
// next to the host root so the toaster and dialog exist.
import { toast } from 'svelte-sonner'
import type { ConfirmOptions, KitNotify } from './context'

export class DefaultNotify implements KitNotify {
  #request = $state<ConfirmOptions | null>(null)
  #open = $state(false)
  #resolve: ((answer: boolean) => void) | null = null

  /** Kept after closing so the dialog keeps its text while it animates out. */
  get request(): ConfirmOptions | null {
    return this.#request
  }

  get open(): boolean {
    return this.#open
  }

  toast(kind: 'info' | 'success' | 'error', text: string): void {
    toast[kind](text)
  }

  /** A newer confirm answers the pending one with `false`. */
  confirm(options: ConfirmOptions): Promise<boolean> {
    this.answer(false)
    this.#request = options
    this.#open = true
    const { promise, resolve } = Promise.withResolvers<boolean>()
    this.#resolve = resolve
    return promise
  }

  answer(isConfirmed: boolean): void {
    this.#open = false
    this.#resolve?.(isConfirmed)
    this.#resolve = null
  }
}

export function createDefaultNotify(): DefaultNotify {
  return new DefaultNotify()
}
