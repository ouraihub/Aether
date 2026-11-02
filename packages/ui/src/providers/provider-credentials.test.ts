import { cleanup, fireEvent, screen, within, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { setup } from './test-support/providers-fixture'

describe('provider credential sessions', () => {
  it('ignores a row reveal after target unmount without copying or showing an old error', async () => {
    const pending = Promise.withResolvers<string>()
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const { notify } = setup({ revealKey: () => pending.promise })
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy key' }))
    cleanup()
    pending.reject(new Error('old secret failure'))
    await pending.promise.catch(() => {})
    await Promise.resolve()
    expect(writeText).not.toHaveBeenCalled()
    expect(notify.toast).not.toHaveBeenCalled()
  })

  it('does not copy or refill a closed and reopened key dialog from its old reveal', async () => {
    const pending = Promise.withResolvers<string>()
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const { notify } = setup({ revealKey: () => pending.promise })
    await fireEvent.click(await screen.findByRole('button', { name: 'Manage API keys' }))
    let dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Copy key' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Close' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    await fireEvent.click(screen.getByRole('button', { name: 'Manage API keys' }))
    dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    pending.resolve('late-plaintext')
    await pending.promise
    await Promise.resolve()
    expect(writeText).not.toHaveBeenCalled()
    expect(dialog.queryByRole('textbox', { name: 'API key' })).toBeNull()
    expect(notify.toast).not.toHaveBeenCalled()
  })

  it('clears the password after reveal but preserves a newer password revision', async () => {
    const pending = Promise.withResolvers<string>()
    const revealKey = vi.fn(() => pending.promise)
    setup({ revealNeedsPassword: true, revealKey })
    await fireEvent.click(await screen.findByRole('button', { name: 'Show key' }))
    const password = screen.getByLabelText('Current password') as HTMLInputElement
    await fireEvent.input(password, { target: { value: 'first-password' } })
    await fireEvent.keyDown(password, { key: 'Enter' })
    await fireEvent.input(password, { target: { value: 'new-password' } })
    pending.reject(new Error('current reveal failed'))
    await pending.promise.catch(() => {})
    await waitFor(() => expect(password.value).toBe('new-password'))
    expect(revealKey).toHaveBeenCalledWith('k1', 'first-password')
  })

  it('clears a successful reveal password before a later copy asks again', async () => {
    setup({ revealNeedsPassword: true, revealKey: async () => 'temporary-plaintext' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Manage API keys' }))
    const dialog = within(await screen.findByRole('dialog', { name: 'Manage API keys' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Edit key' }))
    const password = dialog.getByLabelText('Current password') as HTMLInputElement
    await fireEvent.input(password, { target: { value: 'one-use-password' } })
    await fireEvent.keyDown(password, { key: 'Enter' })
    await waitFor(() =>
      expect((dialog.getByRole('textbox', { name: 'API key' }) as HTMLInputElement).value).toBe(
        'temporary-plaintext',
      ),
    )
    await fireEvent.click(dialog.getByRole('button', { name: 'Cancel' }))
    await fireEvent.click(dialog.getByRole('button', { name: 'Copy key' }))
    expect((dialog.getByLabelText('Current password') as HTMLInputElement).value).toBe('')
  })
})
