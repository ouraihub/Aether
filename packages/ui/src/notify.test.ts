import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import { createI18n } from './i18n'
import KitNotifier from './KitNotifier.svelte'
import { createDefaultNotify } from './notify.svelte'

// No overlay may stay mounted after closing, or an invisible layer blocks the window (docs/16 §6, 陷阱 2).
const overlays = () => document.querySelectorAll('[data-slot$="overlay"]').length

describe('KitNotifier confirm', () => {
  it('resolves false on cancel and unmounts the overlay', async () => {
    const notify = createDefaultNotify()
    render(KitNotifier, { notify, i18n: createI18n('en') })
    const answer = notify.confirm({ title: 'Delete?', body: 'Gone for good.' })
    expect(await screen.findByRole('alertdialog')).toHaveProperty('textContent', expect.stringContaining('Delete?'))
    expect(overlays()).toBe(1)

    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(await answer).toBe(false)
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(overlays()).toBe(0)
  })

  it('resolves true on confirm; a newer confirm answers the older with false', async () => {
    const notify = createDefaultNotify()
    render(KitNotifier, { notify, i18n: createI18n('zh-CN') })
    const first = notify.confirm({ title: 'A', body: '' })
    const second = notify.confirm({ title: 'B', body: '', danger: true })
    expect(await first).toBe(false)

    await fireEvent.click(await screen.findByRole('button', { name: '确定' }))
    expect(await second).toBe(true)
    await waitFor(() => expect(overlays()).toBe(0))
  })
})
