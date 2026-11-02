import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { setup, send } from './test-support/chat-fixture'

describe('ChatPanel markdown', () => {
  it('inserts the finished turn markdown', async () => {
    const onInsert = vi.fn()
    const { runs } = setup(false, null, { props: { onInsert } })
    await send('hi')
    runs[0]?.emit({ kind: 'text_delta', text: 'Hi **there**' })
    expect((await screen.findByRole('button', { name: 'Insert' })).hasAttribute('disabled')).toBe(
      true,
    )
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Insert' }).hasAttribute('disabled')).toBe(false),
    )
    await fireEvent.click(screen.getByRole('button', { name: 'Insert' }))
    expect(onInsert).toHaveBeenCalledWith('Hi **there**')
  })

  it('copies the turn markdown to the clipboard', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    const { runs } = setup(false)
    await send('hi')
    runs[0]?.emit({ kind: 'text_delta', text: '`code`' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await fireEvent.click(await screen.findByRole('button', { name: 'Copy' }))
    expect(writeText).toHaveBeenCalledWith('`code`')
    await screen.findByText('Copied')
  })

  it('offers every eligible reply action and reports clipboard and insertion failures', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: () => Promise.reject(new Error('denied')) },
      configurable: true,
    })
    const { runs, notify } = setup(false, null, {
      props: {
        onInsert: () => {
          throw new Error('read only')
        },
      },
    })
    await send('one')
    runs[0]?.emit({ kind: 'text_delta', text: 'answer one' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await send('two')
    runs[1]?.emit({ kind: 'text_delta', text: 'answer two' })
    runs[1]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('answer two')
    expect(screen.getAllByRole('button', { name: 'Regenerate' })).toHaveLength(2)
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull()
    const copy = screen.getAllByRole('button', { name: 'Copy' })[1]
    if (!copy) throw new Error('Second reply is missing its Copy action')
    await fireEvent.click(copy)
    await waitFor(() => expect(notify.toast).toHaveBeenCalledWith('error', 'Could not copy'))
    const insert = screen.getAllByRole('button', { name: 'Insert' })[1]
    if (!insert) throw new Error('Second reply is missing its Insert action')
    await fireEvent.click(insert)
    expect(notify.toast).toHaveBeenCalledWith('error', 'Internal error')
    expect(screen.getByRole('region', { name: 'Messages' }).hasAttribute('aria-live')).toBe(false)
  })

  it('hides insert without a host capability and hides reasoning for incapable models', async () => {
    const { runs } = setup(false)
    await send('hi')
    runs[0]?.emit({ kind: 'text_delta', text: 'reply' })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('reply')
    expect(screen.queryByRole('button', { name: 'Insert' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Save to note' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Set as context' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Delete' })).toBeNull()
    expect(screen.queryByRole('combobox', { name: 'Reasoning effort' })).toBeNull()
  })

  it('renders streamed Markdown structure while stripping executable HTML and dangerous links', async () => {
    const { runs } = setup(false)
    await send('markdown')
    runs[0]?.emit({
      kind: 'text_delta',
      text: '## Heading\n\n- first\n- second\n\n```ts\nconst safe = true\n```\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n<script>window.hacked = true</script><img src="x" onerror="window.hacked=true"><a href="javascript:alert(1)">unsafe</a>',
    })
    runs[0]?.emit({ kind: 'end', status: 'completed' })
    await screen.findByRole('heading', { name: 'Heading' })
    const messages = screen.getByRole('region', { name: 'Messages' })
    expect(messages.querySelectorAll('ul > li')).toHaveLength(2)
    expect(messages.querySelector('pre code')?.textContent).toContain('const safe = true')
    expect(messages.querySelectorAll('table tbody tr')).toHaveLength(1)
    expect(messages.querySelector('script, [onerror], a[href^="javascript:"]')).toBeNull()
  })
})
