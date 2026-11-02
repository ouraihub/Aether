import { fireEvent, screen, waitFor } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import type { Source } from '../ports'
import { addFrom, mount, preview } from './test-support/skills-fixture'

describe('Skill import sources', () => {
  it('searches a user-selected system folder and previews only the selected authorized candidate', async () => {
    const candidate = {
      name: 'project-writer',
      path: 'E:/workspace/project/.claude/skills/writer',
      origin: 'Selected folder',
    }
    const source: Source = { bytes: { fileName: 'writer.zip', bytes: [7] } }
    const discover = vi.fn(async () => [])
    const pickSearchDirectory = vi.fn(async () => [candidate])
    const prepareSystem = vi.fn(async () => ({ source, origin: candidate.path }))
    const previewFn = vi.fn(async () => preview('system skill body'))
    const install = vi.fn()
    mount({ skillSources: { discover, pickSearchDirectory, prepareSystem }, skills: { preview: previewFn, install } })
    await addFrom('Search this computer')
    await waitFor(() => expect(discover).toHaveBeenCalledTimes(1))
    await fireEvent.click(await screen.findByRole('button', { name: 'Choose skill folder' }))
    await screen.findByText(/E:\/workspace\/project/)
    expect(prepareSystem).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: 'Preview skill' }))
    await screen.findByText('system skill body')
    expect(prepareSystem).toHaveBeenCalledWith(candidate.path, { signal: expect.any(AbortSignal) })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
  })

  it('imports a selected local folder through the same preview boundary without installing', async () => {
    const source: Source = { bytes: { fileName: 'folder.zip', bytes: [4, 5] } }
    const pickDirectory = vi.fn(async () => ({ source, origin: 'E:/skills/writer' }))
    const previewFn = vi.fn(async () => preview('local folder body'))
    const install = vi.fn()
    mount({ skillSources: { pickDirectory }, skills: { preview: previewFn, install } })
    await addFrom('Import local file or folder')
    await fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }))
    await screen.findByText('local folder body')
    expect(pickDirectory).toHaveBeenCalledWith({ signal: expect.any(AbortSignal) })
    expect(previewFn).toHaveBeenCalledWith(source)
    expect(install).not.toHaveBeenCalled()
  })

  it('ignores preparation completed after its source dialog was closed', async () => {
    const pending = Promise.withResolvers<{ source: Source; origin: string }>()
    const pickDirectory = vi.fn(() => pending.promise)
    const previewFn = vi.fn()
    const install = vi.fn()
    mount({ skillSources: { pickDirectory }, skills: { preview: previewFn, install } })
    await addFrom('Import local file or folder')
    await fireEvent.click(screen.getByRole('button', { name: 'Choose folder' }))
    await fireEvent.click(screen.getByRole('button', { name: /^Cancel$/ }))
    pending.resolve({ source: { bytes: { fileName: 'late.zip', bytes: [9] } }, origin: 'late folder' })
    await pending.promise
    await addFrom('Import local file or folder')
    expect(previewFn).not.toHaveBeenCalled()
    expect(install).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: /^Install$/ })).toBeNull()
  })
})
