import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { KitClientError, type ModelOption, type ProvidersPort } from '../ports'
import { setup } from './test-support/chat-fixture'

describe('ChatPanel composer', () => {
  it('uses Ctrl/Cmd+Enter only, protects IME, and refuses duplicate sends', async () => {
    const { runs } = setup(false)
    const input = screen.getByRole('textbox', { name: 'Message' })
    await fireEvent.input(input, { target: { value: 'first\nsecond' } })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(false),
    )
    await fireEvent.keyDown(input, { key: 'Enter' })
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true, isComposing: true })
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true, keyCode: 229 })
    expect(runs).toHaveLength(0)
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true })
    expect(runs).toHaveLength(1)
    expect(runs[0]?.request.input).toBe('first\nsecond')
    await fireEvent.input(input, { target: { value: 'next draft' } })
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    expect(runs).toHaveLength(1)
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    expect(runs).toHaveLength(2)
    expect(runs[1]?.request.input).toBe('next draft')
  })

  it('uses the same model validation for shortcut and send button', async () => {
    const { runs } = setup(false, null, { props: { model: null } })
    const input = screen.getByRole('textbox', { name: 'Message' })
    await fireEvent.input(input, { target: { value: 'draft' } })
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true })
    expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(true)
    expect(runs).toHaveLength(0)
  })

  it('expands the same textarea without losing draft, selection, or input focus', async () => {
    setup(false)
    const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement
    await fireEvent.input(input, { target: { value: 'a multiline\ndraft' } })
    input.focus()
    input.setSelectionRange(2, 7)
    await fireEvent.mouseDown(screen.getByRole('button', { name: 'Expand input' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Expand input' }))
    expect(
      screen.getByRole('button', { name: 'Collapse input' }).getAttribute('aria-expanded'),
    ).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: 'Collapse input' }))
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(input)
    expect(input.value).toBe('a multiline\ndraft')
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 7])
    expect(document.activeElement).toBe(input)
  })

  it('keeps capability loading, failure, retry and actual no-model states distinct', async () => {
    const loading = Promise.withResolvers<ModelOption[]>()
    const modelOptions = vi
      .fn<ProvidersPort['modelOptions']>()
      .mockReturnValueOnce(loading.promise)
      .mockResolvedValueOnce([])
    const { runs } = setup(false, null, { providers: { modelOptions } })
    expect(screen.queryByText('No chat model yet. Add a provider and its models first.')).toBeNull()
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'draft' },
    })
    await fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), {
      key: 'Enter',
      ctrlKey: true,
    })
    expect(runs).toHaveLength(0)
    loading.reject(new KitClientError({ code: 'network', message: 'offline' }))
    await screen.findByText('Network connection failed')
    expect(screen.queryByText('No chat model yet. Add a provider and its models first.')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Retry loading models' }))
    await screen.findByText('No chat model yet. Add a provider and its models first.')
    expect(modelOptions).toHaveBeenCalledTimes(2)
  })
})
