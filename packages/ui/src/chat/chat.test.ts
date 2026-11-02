import { fireEvent, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import { model, setup, send } from './test-support/chat-fixture'

describe('ChatPanel smoke', () => {
  it('streams, stops as cancelled, then regenerates the stored turn', async () => {
    const { runs } = setup(true)
    await send('hello')
    const first = runs[0]
    expect(first?.request).toMatchObject({
      input: 'hello',
      model,
      regenerate: false,
      conversationId: null,
    })
    first?.emit({ kind: 'text_delta', text: 'Hi **there**' })
    await screen.findByText('there')
    await fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    expect(first?.signal?.aborted).toBe(true)
    await screen.findByText('Stopped')
    expect(screen.getAllByText('Stopped')).toHaveLength(1)
    expect(screen.getByText('there')).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Regenerate' })).toHaveLength(1)

    await fireEvent.click(screen.getByRole('button', { name: 'Regenerate' }))
    const second = runs[1]
    expect(second?.request).toMatchObject({
      regenerate: false,
      retryTurnId: runs[0]?.turnId,
      conversationId: 'c1',
      model,
    })
    second?.emit({ kind: 'text_delta', text: 'Again' })
    second?.emit({ kind: 'end', status: 'completed' })
    await screen.findByText('Again')
    expect(screen.getByText('Stopped')).toBeTruthy()
    expect(screen.getByText('there')).toBeTruthy()
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
    expect(screen.getAllByText('hello')).toHaveLength(1)
  })

  it('without history sends fresh runs and hides the conversation list', async () => {
    const { runs } = setup(false)
    expect(screen.queryByRole('button', { name: 'Conversation' })).toBeNull()
    await send('one')
    runs[0]?.emit({ kind: 'end', status: 'failed', error: { code: 'unauthorized', message: 'x' } })
    await screen.findByText('Authentication failed, check the key')
    expect(screen.getAllByRole('button', { name: 'Retry' })).toHaveLength(1)
    expect(screen.queryByRole('button', { name: 'Regenerate' })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(runs[1]?.request).toMatchObject({
      input: 'one',
      regenerate: false,
      conversationId: null,
    })
  })
})
