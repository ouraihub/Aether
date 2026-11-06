// Chat model catalog behind the composer picker (docs/31 §5.1). Provider metadata only enriches
// rows; a failed `list()` never blocks model selection.
import type { KitI18n } from '../i18n'
import type { ModelOption, ModelRef, Provider, ProvidersPort, ReasoningEffort } from '../ports'
import { nearestEffort, REASONING_EFFORTS } from '../providers/reasoning'
import { modelKey, pickerModels, type PickerModel, type PickerValue } from './model-effort'

export class ChatPanelModels {
  options = $state<ModelOption[] | null>(null)
  providers = $state<Provider[] | null>(null)
  error = $state<string | null>(null)
  refreshing = $state(false)
  notice = $state('')
  readonly #port: ProvidersPort | undefined
  readonly #i18n: KitI18n
  readonly #errorText: (error: unknown) => string
  #generation = 0

  constructor(
    port: ProvidersPort | undefined,
    i18n: KitI18n,
    errorText: (error: unknown) => string,
  ) {
    this.#port = port
    this.#i18n = i18n
    this.#errorText = errorText
  }

  get picker(): PickerModel[] {
    return pickerModels(this.options ?? [], this.providers, this.#i18n)
  }

  /** Rereads in place: nulling options would reset the chosen effort (docs/41 §3.5). */
  async load(announce = false): Promise<void> {
    const generation = ++this.#generation
    this.error = null
    if (!this.#port) {
      this.options = []
      return
    }
    this.refreshing = announce
    const [options, providers] = await Promise.allSettled([
      this.#port.modelOptions({ modelType: 'chat' }),
      this.#port.list(),
    ])
    if (generation !== this.#generation) return
    this.refreshing = false
    if (options.status === 'rejected') {
      this.error = this.#errorText(options.reason)
      if (announce) this.notice = this.error
      return
    }
    this.options = options.value
    this.providers = providers.status === 'fulfilled' ? providers.value : null
    if (announce)
      this.notice = this.#i18n.t('chat.picker.reloaded', { count: options.value.length })
  }

  #find(key: string): ModelOption | undefined {
    return this.options?.find((item) => modelKey(item.endpointId, item.modelId) === key)
  }

  /** Hands the host the model behind the key; a failed save is shown in the picker notice. */
  async saveDefault(
    value: PickerValue,
    save: (model: ModelRef, effort: ReasoningEffort) => Promise<void> | void,
  ): Promise<void> {
    const option = this.#find(value.key)
    if (!option) return
    try {
      await save({ endpointId: option.endpointId, modelId: option.modelId }, value.effort)
    } catch (error) {
      this.notice = this.#errorText(error)
    }
  }

  /**
   * A model switch keeps the saved effort policy (nearest allowed level); only a level the user
   * picked on the same model, or typed as a search suffix, is written back.
   */
  select(
    value: PickerValue,
    picked: ModelRef | null,
    chosen: ReasoningEffort,
    pick: (ref: ModelRef) => void,
    choose: (effort: ReasoningEffort) => void,
  ): void {
    const option = this.#find(value.key)
    if (!option) return
    const same = picked !== null && modelKey(picked.endpointId, picked.modelId) === value.key
    if (!same) pick({ endpointId: option.endpointId, modelId: option.modelId })
    const declared = option.reasoning
    const allowed = REASONING_EFFORTS.filter((effort) => declared?.efforts.includes(effort))
    const implied = nearestEffort(chosen, allowed)
    if (value.effort !== implied && (same || value.effort !== 'default')) choose(value.effort)
  }
}
