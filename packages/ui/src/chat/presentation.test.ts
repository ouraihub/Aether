import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { createRawSnippet } from 'svelte'
import { describe, expect, it, vi } from 'vitest'
import ChatComposer from './ChatComposer.svelte'
import ChatMessage from './ChatMessage.svelte'
import UsageControl from './UsageControl.svelte'
import { createI18n } from '../i18n'

describe('standalone chat presentation', () => {
  it('keeps unknown context unknown, clamps only the meter, and restores focus on Escape', async () => {
    const i18n = createI18n('en-US')
    const view = render(UsageControl, {
      props: { i18n, context: { tokens: null, contextWindow: 1000, modelName: 'Model A', source: 'unknown' } },
    })
    const trigger = screen.getByRole('button', { name: 'Usage and settings' })
    await fireEvent.click(trigger)
    expect(screen.queryByRole('meter')).toBeNull()
    expect(screen.getByText('Unknown / 1,000')).toBeTruthy()
    await view.rerender({
      i18n,
      context: { tokens: 1500, contextWindow: 1000, modelName: 'Model A', source: 'reported' },
    })
    expect(screen.getByRole('meter').getAttribute('value')).toBe('100')
    expect(screen.getByText(/1,500 \/ 1,000/)).toBeTruthy()
    await fireEvent.keyDown(screen.getByRole('meter'), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('meter')).toBeNull())
    await waitFor(() => expect(document.activeElement).toBe(trigger))
  })
  it('guards the actual composer shortcut and preserves its draft, selection and focus through an override', async () => {
    const onsubmit = vi.fn()
    const props = { value: 'keep this draft', label: 'Message', submitLabel: 'Send', onsubmit }
    const view = render(ChatComposer, { props })
    const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement
    input.focus()
    input.setSelectionRange(2, 6)
    await fireEvent.keyDown(input, { key: 'Enter' })
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true, isComposing: true })
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true, keyCode: 229 })
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true, repeat: true })
    expect(onsubmit).not.toHaveBeenCalled()
    await fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true })
    expect(onsubmit).toHaveBeenCalledTimes(1)
    await fireEvent.mouseDown(screen.getByRole('button', { name: 'Expand input' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Expand input' }))
    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    await fireEvent(input, escape)
    expect(escape.defaultPrevented).toBe(true)
    expect(screen.getByRole('button', { name: 'Expand input' }).getAttribute('aria-expanded')).toBe('false')
    const override = createRawSnippet(() => ({ render: () => '<button type="button">Approve</button>' }))
    await view.rerender({ ...props, override })
    expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull()
    screen.getByRole('button', { name: 'Approve' }).focus()
    await view.rerender({ ...props, override: undefined })
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(input)
    expect(input.value).toBe('keep this draft')
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 6])
    await waitFor(() => expect(document.activeElement).toBe(input))
    await view.rerender({ ...props, disabled: true })
    await fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    expect(onsubmit).toHaveBeenCalledTimes(1)
  })

  it('discloses user text by nonempty logical lines rather than wrapped visual lines', async () => {
    const text = 'one\n\ntwo\nthree\nfour\nfive\nsix'
    render(ChatMessage, { props: { role: 'user', text } })
    expect(screen.queryByText(/six/)).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Show more' }))
    expect(screen.getByText(/six/).textContent).toBe(text)
    await fireEvent.click(screen.getByRole('button', { name: 'Show less' }))
    expect(screen.queryByText(/six/)).toBeNull()
  })
})
