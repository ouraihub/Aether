import { render, screen } from '@testing-library/svelte'
import { expect, it } from 'vitest'
import { createI18n } from '../i18n'
import ModelEffortPicker from './ModelEffortPicker.svelte'

it('lists a host model’s levels from off to maximum whatever order it declares', () => {
  render(ModelEffortPicker, {
    models: [
      {
        key: 'm',
        providerId: 'p',
        provider: 'P',
        name: 'Model',
        detail: '',
        hasKey: false,
        efforts: ['low', 'medium', 'high', 'max', 'xhigh', 'minimal', 'none'],
        contextWindow: null,
      },
    ],
    value: { key: 'm', effort: 'default' },
    i18n: createI18n('en'),
    open: true,
    onselect: () => {},
  })
  const levels = screen
    .getAllByRole('radio')
    .map((item) => item.getAttribute('aria-label')?.replace(/^Reasoning effort /, ''))
  expect(levels).toEqual(['None', 'Minimal', 'Low', 'Medium', 'High', 'Very high', 'Maximum'])
})
