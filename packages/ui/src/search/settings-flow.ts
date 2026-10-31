import { KitClientError } from '../ports/contract'
import type { CallOptions } from '../ports/contract'
import type { SearchPort, SearchSave, SearchSettingsDto } from '../ports/search'

/** Applies each persisted step before attempting the next (docs/30 §4). */
export async function saveSearchSettings(
  port: SearchPort,
  input: {
    config: SearchSave
    key?: { value: string; keyId?: string }
    check: boolean
  },
  session: {
    isCurrent(): boolean
    onSettings(next: SearchSettingsDto): void
    onConfigSaved(sent: SearchSave): void
    onKeySaved(): void
  },
  options?: CallOptions,
): Promise<{ settings: SearchSettingsDto; checked?: { count: number } }> {
  const config = { ...input.config }
  const key = input.key ? { ...input.key } : undefined
  const check = input.check
  const cancelled = () =>
    new KitClientError({
      code: 'cancelled',
      message: 'Search settings cancelled',
      details: {},
    })
  function assertCurrent() {
    if (options?.signal?.aborted || !session.isCurrent()) throw cancelled()
  }
  // Consume an in-flight response even if its transport does not honor abort.
  async function step<T>(call: () => Promise<T>): Promise<T> {
    assertCurrent()
    const signal = options?.signal
    let onAbort: (() => void) | undefined
    try {
      const operation = call()
      let value: T
      if (signal) {
        const abort = Promise.withResolvers<never>()
        onAbort = () => abort.reject(cancelled())
        signal.addEventListener('abort', onAbort, { once: true })
        if (signal.aborted) onAbort()
        value = await Promise.race([operation, abort.promise])
      } else value = await operation
      assertCurrent()
      return value
    } catch (error) {
      assertCurrent()
      throw error
    } finally {
      if (signal && onAbort) signal.removeEventListener('abort', onAbort)
    }
  }

  let settings = await step(() => port.save(config, options))
  session.onSettings(settings)
  assertCurrent()
  session.onConfigSaved(config)
  if (key?.value.trim()) {
    settings = await step(() =>
      port.saveKey(
        config.capability,
        config.provider,
        key.value,
        key.keyId,
        options,
      ),
    )
    session.onSettings(settings)
    assertCurrent()
    session.onKeySaved()
  }
  const checked = check
    ? await step(() => port.check(config.capability, config.provider, options))
    : undefined
  assertCurrent()
  return checked ? { settings, checked } : { settings }
}
