import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError, type ModelOption, type ReasoningEffort } from '../ports'
import { model, pickerTrigger, setup, send } from './test-support/chat-fixture'

const option = (modelId: string, efforts: ReasoningEffort[]): ModelOption => ({
  endpointId: model.endpointId,
  modelId,
  label: `Local · main · ${modelId}`,
  name: null,
  modelType: 'chat',
  supportsTools: null,
  reasoning: { dialect: 'openai', efforts },
  nativeWebSearch: false,
  canVision: false,
})
const SEARCH = 'Search models, optionally with a level'

describe('ChatPanel composer picker', () => {
  it('picks a model with a typed level and saves it as the host default', async () => {
    const onDefaultModelChange = vi.fn<(ref: unknown, effort: ReasoningEffort) => void>()
    const modelOptions = vi.fn(() =>
      Promise.resolve([option('m1', ['low', 'high']), option('m2', ['none', 'medium'])]),
    )
    const { runs } = setup(false, null, {
      props: { defaultModel: model, onDefaultModelChange },
      providers: { modelOptions },
    })
    await fireEvent.click(await pickerTrigger())
    await waitFor(() => expect(modelOptions).toHaveBeenCalledTimes(2))
    const setDefault = screen.getByRole('button', { name: 'Set as default' })
    expect(setDefault.hasAttribute('disabled')).toBe(true)
    await fireEvent.input(screen.getByLabelText(SEARCH), { target: { value: 'm2 none' } })
    expect(screen.getAllByRole('option')).toHaveLength(1)
    await fireEvent.click(screen.getByRole('option', { name: /^Select m2 / }))
    await screen.findByRole('button', { name: 'Model and reasoning: m2 · None' })
    await waitFor(() => expect(setDefault.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(setDefault)
    expect(onDefaultModelChange).toHaveBeenCalledWith(
      { endpointId: model.endpointId, modelId: 'm2' },
      'none',
    )
    await fireEvent.keyDown(screen.getByLabelText(SEARCH), { key: 'Escape' })
    await send('after default')
    expect(runs[0]?.request).toMatchObject({
      model: { endpointId: model.endpointId, modelId: 'm2' },
      reasoningEffort: 'none',
    })
  })

  it('reports a failed default save in the picker without changing the selection', async () => {
    const onDefaultModelChange = vi.fn(() =>
      Promise.reject(new KitClientError({ code: 'network', message: 'offline' })),
    )
    setup(false, null, {
      props: { onDefaultModelChange },
      providers: { modelOptions: () => Promise.resolve([option('m1', ['low'])]) },
    })
    await fireEvent.click(await pickerTrigger())
    await fireEvent.click(await screen.findByRole('button', { name: 'Set as default' }))
    expect(await screen.findByRole('status')).toBeTruthy()
    expect(screen.getByRole('status').textContent).not.toBe('')
    expect(screen.getByRole('button', { name: 'Model and reasoning: m1 · Default' })).toBeTruthy()
  })

  it('allows saving an effort-only change on the default model', async () => {
    const save = vi.fn()
    setup(false, null, {
      props: {
        defaultModel: model,
        defaultReasoningEffort: 'low',
        reasoningEffort: 'high',
        onDefaultModelChange: save,
      },
      providers: { modelOptions: () => Promise.resolve([option('m1', ['low', 'high'])]) },
    })
    await fireEvent.click(await pickerTrigger())
    const action = await screen.findByRole('button', { name: 'Set as default' })
    expect(action.hasAttribute('disabled')).toBe(false)
    await fireEvent.click(action)
    expect(save).toHaveBeenCalledWith(model, 'high')
  })

  it('hides set-as-default when the host cannot persist a default', async () => {
    setup(false)
    await fireEvent.click(await pickerTrigger())
    await screen.findByLabelText(SEARCH)
    expect(screen.queryByRole('button', { name: 'Set as default' })).toBeNull()
  })
})
