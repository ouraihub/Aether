import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { KitClientError, type Conversation, type ModelOption, type RunClient } from '../ports'
import { model, required, setup, send, type Extra } from './test-support/chat-fixture'

describe('ChatPanel images', () => {
  const visionModel = { endpointId: 'e2', modelId: 'v1' }
  const option = (ref: typeof model, label: string, canVision: boolean): ModelOption => ({
    ...ref,
    label,
    name: null,
    modelType: 'chat',
    supportsTools: null,
    reasoning: null,
    nativeWebSearch: false,
    canVision,
  })
  const textOption = option(model, 'Text model', false)
  const visionOption = option(visionModel, 'Vision model', true)
  const png = (name = 'a.png', size = 3) =>
    new File([new Uint8Array(size).fill(65)], name, { type: 'image/png' })
  const revoke = vi.fn()
  let created = 0
  beforeEach(() => {
    created = 0
    revoke.mockClear()
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      value: () => `blob:image-${++created}`,
    })
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revoke })
  })
  function visionSetup(extra: Extra = {}) {
    return setup(true, null, {
      ...extra,
      props: { model: visionModel, ...extra.props },
      providers: {
        modelOptions: () => Promise.resolve([textOption, visionOption]),
        ...extra.providers,
      },
    })
  }
  async function uploadItem() {
    await fireEvent.click(screen.getByRole('button', { name: 'More tools' }))
    return screen.findByRole('button', { name: /^Upload attachment/ })
  }
  async function pick(files: File[]) {
    const upload = await uploadItem()
    await waitFor(() => expect(upload.hasAttribute('disabled')).toBe(false))
    await fireEvent.click(upload)
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /^Upload attachment/ })).toBeNull(),
    )
    const input = required(document.querySelector<HTMLInputElement>('input[type=file]'))
    expect(input.accept).toBe('image/png,image/jpeg,image/webp,image/gif')
    await fireEvent.change(input, { target: { files } })
  }

  it('attaches, removes and sends images, then clears the tray and shows them on the turn', async () => {
    const { runs } = visionSetup()
    await pick([png('a.png'), png('b.png')])
    await fireEvent.click(await screen.findByRole('button', { name: 'Remove image 2' }))
    expect(revoke).toHaveBeenCalledWith('blob:image-2')
    expect(screen.queryByRole('button', { name: 'Remove image 2' })).toBeNull()
    await send('What is this?')
    expect(runs[0]?.request.images).toEqual([{ mime: 'image/png', data: 'QUFB' }])
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Remove image 1' })).toBeNull())
    const link = screen.getByRole('link', { name: 'Open image 1' })
    expect(link.getAttribute('target')).toBe('_blank')
    expect(link.getAttribute('rel')).toBe('noopener')
    expect(link.getAttribute('href')).toBe('data:image/png;base64,QUFB')
    expect(revoke).toHaveBeenCalledWith('blob:image-1')
  })

  it('takes pasted images and leaves plain text paste alone', async () => {
    const { runs } = visionSetup()
    const input = screen.getByRole('textbox', { name: 'Message' })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy())
    expect(await fireEvent.paste(input, { clipboardData: { files: [] } })).toBe(true)
    expect(await fireEvent.paste(input, { clipboardData: { files: [png()] } })).toBe(false)
    await screen.findByRole('button', { name: 'Remove image 1' })
    await send('pasted')
    expect(runs[0]?.request.images).toHaveLength(1)
  })

  it('accepts dropped images and rejects over-limit, wrong type and oversized files without queueing', async () => {
    const { notify } = visionSetup()
    await pick([png('1.png'), png('2.png'), png('3.png')])
    await screen.findByRole('button', { name: 'Remove image 3' })
    const form = required(screen.getByRole('textbox', { name: 'Message' }).closest('form'))
    await fireEvent.drop(form, {
      dataTransfer: { files: [png('4.png'), png('5.png')], types: ['Files'] },
    })
    await waitFor(() =>
      expect(notify.toast).toHaveBeenCalledWith('error', 'Up to 4 images per message'),
    )
    await fireEvent.drop(form, {
      dataTransfer: { files: [new File(['x'], 'x.bmp', { type: 'image/bmp' })] },
    })
    expect(notify.toast).toHaveBeenCalledWith(
      'error',
      'Only PNG, JPEG, WebP and GIF images are supported',
    )
    await fireEvent.drop(form, { dataTransfer: { files: [png('big.png', 4 * 1024 * 1024 + 1)] } })
    expect(notify.toast).toHaveBeenCalledWith('error', 'Each image must be 4 MiB or smaller')
    expect(screen.queryByRole('button', { name: 'Remove image 4' })).toBeNull()
    await fireEvent.drop(form, { dataTransfer: { files: [png('4.png')] } })
    await screen.findByRole('button', { name: 'Remove image 4' })
  })

  it('disables attach and send for a non-vision model until a vision model is picked', async () => {
    const { notify } = visionSetup({ props: { model } })
    const upload = await uploadItem()
    await waitFor(() =>
      expect(upload.getAttribute('title')).toBe('The current model does not support images'),
    )
    expect(upload.hasAttribute('disabled')).toBe(true)
    await fireEvent.keyDown(upload, { key: 'Escape' })
    await fireEvent.paste(screen.getByRole('textbox', { name: 'Message' }), {
      clipboardData: { files: [png()] },
    })
    await waitFor(() =>
      expect(notify.toast).toHaveBeenCalledWith(
        'error',
        'The current model does not support images',
      ),
    )
    expect(screen.queryByRole('button', { name: 'Remove image 1' })).toBeNull()
    await fireEvent.click(screen.getByLabelText('Select a model'))
    await fireEvent.click(await screen.findByRole('option', { name: /v1/ }))
    await pick([png()])
    await screen.findByRole('button', { name: 'Remove image 1' })
    await fireEvent.input(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'hi' },
    })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(false),
    )
    await fireEvent.click(screen.getByLabelText('Select a model'))
    await fireEvent.click(await screen.findByRole('option', { name: /m1/ }))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Send' }).hasAttribute('disabled')).toBe(true),
    )
    expect(screen.getByRole('status', { name: '' }).textContent).toContain(
      'does not support images',
    )
  })

  it('restores images with the draft when the request is rejected before acceptance', async () => {
    const run = vi
      .fn<RunClient['start']>()
      .mockRejectedValueOnce(new KitClientError({ code: 'network', message: 'offline' }))
    visionSetup({ chat: { start: run } })
    await pick([png()])
    await screen.findByRole('button', { name: 'Remove image 1' })
    await send('keep my image')
    expect(run.mock.calls[0]?.[0].images).toEqual([{ mime: 'image/png', data: 'QUFB' }])
    await waitFor(() =>
      expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe(
        'keep my image',
      ),
    )
    expect(screen.getByRole('button', { name: 'Remove image 1' })).toBeTruthy()
    expect(revoke).not.toHaveBeenCalled()
  })

  it('shows expired history images and lists only vision models for @ when the source has images', async () => {
    const turn = (id: string): Conversation['turns'][number] => ({
      id,
      parentId: null,
      questionId: 'q',
      supersedesId: null,
      kind: 'reply',
      ...model,
      input: 'Shared question',
      text: `Reply ${id}`,
      status: 'completed',
      errorCode: null,
      reasoningEffort: 'default',
      reasoningText: null,
      images: [
        { mime: 'image/png', url: null },
        { mime: 'image/png', url: '/img/1' },
      ],
      generated: [],
    })
    const conversation: Conversation = {
      summary: {
        id: 'saved',
        title: 'Images',
        scope: null,
        selectedTurnId: 'a',
        createdAt: 0,
        updatedAt: 0,
      },
      turns: [turn('a'), turn('b')],
    }
    visionSetup({
      props: { conversationId: 'saved' },
      history: { get: () => Promise.resolve(conversation) },
    })
    expect(await screen.findByText('Image expired')).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Open image 2' }).getAttribute('href')).toBe('/img/1')
    await waitFor(() =>
      expect(
        screen.getAllByRole('button', { name: 'Switch model' })[0]?.hasAttribute('disabled'),
      ).toBe(false),
    )
    await fireEvent.click(required(screen.getAllByRole('button', { name: 'Switch model' })[0]))
    await screen.findByRole('option', { name: /v1/ })
    expect(screen.queryByRole('option', { name: /m1/ })).toBeNull()
  })

  it('saves an image-only reply to a note together with its generated images', async () => {
    const generated = [{ mime: 'image/png', url: 'data:image/png;base64,AAAA' }]
    const conversation: Conversation = {
      summary: {
        id: 'saved',
        title: 'Painting',
        scope: null,
        selectedTurnId: 'a',
        createdAt: 0,
        updatedAt: 0,
      },
      turns: [
        {
          id: 'a',
          parentId: null,
          questionId: 'q',
          supersedesId: null,
          kind: 'reply',
          ...model,
          input: 'Draw a cat',
          text: '',
          status: 'completed',
          errorCode: null,
          reasoningEffort: 'default',
          reasoningText: null,
          images: [],
          generated,
        },
      ],
    }
    const onSaveNote = vi.fn()
    visionSetup({
      props: { conversationId: 'saved', onSaveNote },
      history: { get: () => Promise.resolve(conversation) },
    })
    await fireEvent.click(await screen.findByRole('button', { name: 'Save to note' }))
    expect(onSaveNote).toHaveBeenCalledWith('', generated)
  })
})
