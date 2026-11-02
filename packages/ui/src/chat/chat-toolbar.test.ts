import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import type { ModelOption, ProvidersPort } from '../ports'
import { model, setup, send } from './test-support/chat-fixture'
import { providersRevision, settingsTarget } from '../providers/revision.svelte'
import { nearestEffort } from '../providers/reasoning'

describe('ChatPanel toolbar and model selection', () => {
  it('reloads model capabilities after providers settings change', async () => {
    const option = {
      ...model,
      label: 'Local · main · m1',
      modelType: 'chat',
      name: null,
      supportsTools: null,
      reasoning: null,
      nativeWebSearch: false,
    } as const
    const modelOptions = vi
      .fn<ProvidersPort['modelOptions']>()
      .mockResolvedValueOnce([{ ...option, canVision: false }])
      .mockResolvedValue([{ ...option, canVision: true }])
    setup(false, null, { providers: { modelOptions } })
    await fireEvent.click(await screen.findByLabelText('Select a model'))
    expect(screen.queryByRole('button', { name: 'Vision' })).toBeNull()
    await fireEvent.keyDown(screen.getByPlaceholderText('Search models…'), { key: 'Escape' })
    providersRevision.value++
    await waitFor(() => expect(modelOptions).toHaveBeenCalledTimes(2))
    await fireEvent.click(await screen.findByLabelText('Select a model'))
    expect(await screen.findByRole('button', { name: 'Vision' })).toBeTruthy()
  })

  it('keeps the chosen effort across a provider save and drops generate image once tools are unsupported', async () => {
    const option: ModelOption = {
      ...model,
      label: 'Local · main · m1',
      name: 'Mine',
      modelType: 'chat',
      supportsTools: null,
      reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
      nativeWebSearch: false,
      canVision: false,
    }
    const modelOptions = vi
      .fn<ProvidersPort['modelOptions']>()
      .mockResolvedValueOnce([option])
      .mockResolvedValue([{ ...option, supportsTools: false }])
    const { runs } = setup(false, null, {
      providers: { modelOptions },
      props: { paintingModel: true, toolbar: ['generate-image'] },
    })
    expect(await screen.findByText('Mine')).toBeTruthy()
    await fireEvent.click(await screen.findByRole('button', { name: 'Reasoning effort: Default' }))
    await fireEvent.input(await screen.findByRole('slider', { name: 'Reasoning effort' }), {
      target: { value: '1' },
    })
    await fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' })
    const generate = screen.getByRole('button', { name: 'Generate image' })
    await fireEvent.click(generate)
    expect(generate.getAttribute('aria-pressed')).toBe('true')
    providersRevision.value++
    await waitFor(() => expect(modelOptions).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(generate.hasAttribute('disabled')).toBe(true))
    expect(generate.getAttribute('aria-pressed')).toBe('false')
    expect(screen.getByRole('button', { name: 'Reasoning effort: High' })).toBeTruthy()
    await send('after save')
    expect(runs[0]?.request).toMatchObject({ reasoningEffort: 'high' })
    expect(runs[0]?.request.generateImage).toBeUndefined()
  })

  it('without chat models offers to open settings', async () => {
    const onOpenSettings = vi.fn()
    setup(false, null, {
      providers: { modelOptions: () => Promise.resolve([]) },
      props: { onOpenSettings },
    })
    await fireEvent.click(await screen.findByRole('button', { name: 'Open settings' }))
    expect(onOpenSettings).toHaveBeenCalledOnce()
  })

  it('Esc closes the panel unless a run is active', async () => {
    const onClose = vi.fn()
    setup(false, null, { props: { onClose } })
    const input = screen.getByRole('textbox', { name: 'Message' })
    await send('hi')
    await fireEvent.keyDown(input, { key: 'Escape' })
    expect(onClose).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await fireEvent.keyDown(input, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('sends explicit web search and keeps sources on its turn', async () => {
    const { runs } = setup(false, null, { search: {} })
    await fireEvent.click(screen.getByRole('button', { name: 'Web search' }))
    await send('latest news')
    expect(runs[0]?.request).toMatchObject({ webSearch: true, input: 'latest news' })
    runs[0]?.emit({ kind: 'search_start' })
    runs[0]?.emit({
      kind: 'search_result',
      query: 'latest news',
      provider: 'jina',
      hits: [{ title: 'Example', url: 'https://example.com', snippet: 'Result' }],
    })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByRole('link', { name: 'Example' })
    expect(screen.getByRole('link', { name: 'Example' }).getAttribute('href')).toBe(
      'https://example.com',
    )
    expect(screen.getByRole('button', { name: 'Web search' }).getAttribute('aria-pressed')).toBe(
      'true',
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Web search' }))
    await send('do not search')
    expect(runs[1]?.request.webSearch).not.toBe(true)
    runs[1]?.emit({ kind: 'end', status: 'completed' })
  })

  it('customizes the toolbar: toggles, keyboard reorder, restore default (docs/38)', async () => {
    const onToolbarChange = vi.fn()
    setup(false, null, { search: {}, props: { onToolbarChange } })
    const toolbarLabels = () =>
      [...document.querySelectorAll('.composer-tools .tool-group:first-child > button')].map((b) =>
        b.getAttribute('aria-label'),
      )
    expect(toolbarLabels()).toEqual(['New conversation', 'Web search', 'More tools'])
    await fireEvent.click(screen.getByRole('button', { name: 'More tools' }))
    expect(await screen.findByRole('button', { name: /^Upload attachment/ })).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Web search' })).toHaveLength(1)
    await fireEvent.click(screen.getByRole('button', { name: 'Customize toolbar' }))
    await fireEvent.click(await screen.findByRole('switch', { name: 'Upload attachment' }))
    expect(onToolbarChange).toHaveBeenLastCalledWith([
      'new-conversation',
      'web-search',
      'attachment',
    ])
    await fireEvent.keyDown(
      screen.getByRole('button', { name: 'Drag or use ↑/↓ to reorder Upload attachment' }),
      {
        key: 'ArrowUp',
      },
    )
    expect(onToolbarChange).toHaveBeenLastCalledWith([
      'new-conversation',
      'attachment',
      'web-search',
    ])
    await fireEvent.click(screen.getByRole('switch', { name: 'New conversation' }))
    expect(onToolbarChange).toHaveBeenLastCalledWith(['attachment', 'web-search'])
    expect(toolbarLabels()).toEqual(['Upload attachment', 'Web search', 'More tools'])
    await fireEvent.click(screen.getByRole('button', { name: 'Restore default' }))
    expect(onToolbarChange).toHaveBeenLastCalledWith(['new-conversation', 'web-search'])
    expect(screen.getByRole('button', { name: 'Restore default' }).hasAttribute('disabled')).toBe(
      true,
    )
  })

  it('exposes only configured reasoning choices and sends default without changing its meaning', async () => {
    const { runs } = setup(false, null, {
      providers: {
        modelOptions: () =>
          Promise.resolve([
            {
              ...model,
              label: 'Reasoner',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['high'] },
              nativeWebSearch: false,
              canVision: false,
            },
          ]),
      },
    })
    const effort = await screen.findByRole('button', { name: 'Reasoning effort: Default' })
    await fireEvent.click(effort)
    expect(await screen.findByLabelText('High')).toBeTruthy()
    expect(screen.queryByLabelText('Medium')).toBeNull()
    expect(screen.queryByRole('slider')).toBeNull()
    expect((screen.getByLabelText('Default') as HTMLInputElement).checked).toBe(true)
    await send('default request')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
    await waitFor(() => expect(effort.hasAttribute('disabled')).toBe(true))
    expect((screen.getByLabelText('High') as HTMLInputElement).matches(':disabled')).toBe(true)
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(effort.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(screen.getByLabelText('High'))
    await send('explicit request')
    expect(runs[1]?.request.reasoningEffort).toBe('high')
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Reset to default' }).hasAttribute('disabled'),
      ).toBe(false),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Reset to default' }))
    await send('reset request')
    expect(runs[2]?.request.reasoningEffort).toBe('default')
  })

  it('maps sparse slider indices exactly, preserves default on open, and isolates Escape', async () => {
    const onClose = vi.fn()
    const onReasoningEffortChange = vi.fn()
    const { runs } = setup(false, null, {
      props: { onClose, onReasoningEffortChange },
      providers: {
        modelOptions: () =>
          Promise.resolve([
            {
              ...model,
              label: 'Sparse reasoner',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['high', 'none', 'low'] },
              nativeWebSearch: false,
              canVision: false,
            },
            {
              endpointId: model.endpointId,
              modelId: 'm2',
              label: 'Other reasoner',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
              nativeWebSearch: false,
              canVision: false,
            },
            {
              endpointId: model.endpointId,
              modelId: 'm3',
              label: 'Without reasoning',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: null,
              nativeWebSearch: false,
              canVision: false,
            },
          ]),
      },
    })
    const trigger = await screen.findByRole('button', { name: 'Reasoning effort: Default' })
    await fireEvent.click(trigger)
    let slider = (await screen.findByRole('slider', {
      name: 'Reasoning effort',
    })) as HTMLInputElement
    expect(slider.value).toBe('1')
    expect(slider.max).toBe('2')
    expect(slider.getAttribute('aria-valuetext')).toBe('Default')
    await fireEvent.keyDown(slider, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('slider')).toBeNull())
    expect(onClose).not.toHaveBeenCalled()
    await waitFor(() => expect(document.activeElement).toBe(trigger))
    await send('opened default')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() => expect(trigger.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(trigger)
    slider = (await screen.findByRole('slider', { name: 'Reasoning effort' })) as HTMLInputElement
    for (const [index, effort] of ['none', 'low', 'high'].entries()) {
      await fireEvent.input(slider, { target: { value: String(index) } })
      await send(`effort ${effort}`)
      expect(runs[index + 1]?.request.reasoningEffort).toBe(effort)
      await waitFor(() => expect(slider.disabled).toBe(true))
      runs[index + 1]?.emit({ kind: 'end', status: 'completed' })
      await waitFor(() => expect(slider.disabled).toBe(false))
    }
    await fireEvent.keyDown(slider, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('slider')).toBeNull())
    expect(onReasoningEffortChange).toHaveBeenLastCalledWith('high')
    const models = screen.getByLabelText('Select a model')
    const pick = async (name: RegExp) => {
      await fireEvent.click(models)
      await fireEvent.click(await screen.findByRole('option', { name }))
    }
    // The saved choice follows the model: kept where allowed, dropped where unsupported, restored on return.
    await pick(/m2/)
    expect(await screen.findByRole('button', { name: 'Reasoning effort: High' })).toBeTruthy()
    await send('changed model')
    expect(runs[4]?.request).toMatchObject({
      model: { endpointId: model.endpointId, modelId: 'm2' },
      reasoningEffort: 'high',
    })
    runs[4]?.emit({ kind: 'end', status: 'completed' })
    await pick(/m3/)
    expect(await screen.findByRole('button', { name: 'Reasoning effort: Default' })).toBeTruthy()
    await send('no reasoning')
    expect(runs[5]?.request.reasoningEffort).toBe('default')
    runs[5]?.emit({ kind: 'end', status: 'completed' })
    await pick(/m1/)
    expect(await screen.findByRole('button', { name: 'Reasoning effort: High' })).toBeTruthy()
  })

  it('falls back to the nearest allowed effort, preferring the higher one on a tie', () => {
    expect(nearestEffort('high', ['none', 'low'])).toBe('low')
    expect(nearestEffort('medium', ['low', 'high'])).toBe('high')
    expect(nearestEffort('max', ['low', 'medium', 'high'])).toBe('high')
    expect(nearestEffort('high', [])).toBe('default')
    expect(nearestEffort('default', ['low'])).toBe('default')
  })

  it('starts from the saved effort choice', async () => {
    const { runs } = setup(false, null, {
      props: { reasoningEffort: 'high' },
      providers: {
        modelOptions: () =>
          Promise.resolve([
            {
              ...model,
              label: 'Reasoner',
              modelType: 'chat',
              name: null,
              supportsTools: null,
              reasoning: { dialect: 'openai', efforts: ['none', 'low'] },
              nativeWebSearch: false,
              canVision: false,
            },
          ]),
      },
    })
    expect(await screen.findByRole('button', { name: 'Reasoning effort: Low' })).toBeTruthy()
    await send('restored')
    expect(runs[0]?.request.reasoningEffort).toBe('low')
  })

  it('groups models by provider, searches, filters by capability and links to settings', async () => {
    const onOpenSettings = vi.fn()
    const onClose = vi.fn()
    const base = {
      name: null,
      modelType: 'chat',
      supportsTools: null,
      reasoning: null,
      nativeWebSearch: false,
      canVision: false,
    } as const
    setup(false, null, {
      props: { onOpenSettings, onClose },
      providers: {
        modelOptions: () =>
          Promise.resolve([
            { ...base, ...model, label: 'Local · main · m1', supportsTools: true },
            {
              ...base,
              endpointId: 'e2',
              modelId: 'm2',
              label: 'Cloud · x · m2',
              reasoning: { dialect: 'openai', efforts: ['low', 'high'] },
              canVision: true,
            },
          ]),
      },
    })
    await fireEvent.click(await screen.findByLabelText('Select a model'))
    expect(await screen.findByText('Local · main')).toBeTruthy()
    expect(screen.getByText('Cloud · x')).toBeTruthy()
    expect(screen.getAllByRole('option')).toHaveLength(2)
    await fireEvent.input(screen.getByPlaceholderText('Search models…'), {
      target: { value: 'cloud' },
    })
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([
        expect.stringContaining('m2'),
      ]),
    )
    await fireEvent.keyDown(screen.getByPlaceholderText('Search models…'), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('option')).toBeNull())
    expect(onClose).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByLabelText('Select a model'))
    await screen.findAllByRole('option')
    await fireEvent.click(screen.getByRole('button', { name: 'Vision' }))
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([
        expect.stringContaining('m2'),
      ]),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Vision' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Tools' }))
    await waitFor(() =>
      expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([
        expect.stringContaining('m1'),
      ]),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Configure models' }))
    expect(onOpenSettings).toHaveBeenCalledOnce()
    await waitFor(() => expect(screen.queryByRole('option')).toBeNull())
  })

  it('keeps reasoning discoverable without sending unsupported efforts', async () => {
    const onOpenSettings = vi.fn()
    const { runs } = setup(false, null, { props: { onOpenSettings } })
    await fireEvent.click(await screen.findByRole('button', { name: 'Reasoning effort: Default' }))
    expect(await screen.findByText(/No reasoning capabilities are configured/)).toBeTruthy()
    expect(screen.queryByRole('slider')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Open settings' }))
    expect(onOpenSettings).toHaveBeenCalledOnce()
    expect(settingsTarget.value).toEqual(model)
    settingsTarget.value = null
    await send('provider default')
    expect(runs[0]?.request.reasoningEffort).toBe('default')
  })
})
