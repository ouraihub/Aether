import { describe, expect, it } from 'vitest'
import { en } from './en'
import { createI18n } from './index'
import { zhCN } from './zh-CN'

describe('createI18n', () => {
  it('picks Chinese for zh locales and English otherwise', () => {
    expect(createI18n('zh-CN').t('common.cancel')).toBe('取消')
    expect(createI18n('zh-TW').t('common.cancel')).toBe('取消')
    expect(createI18n('en-US').t('common.cancel')).toBe('Cancel')
    expect(createI18n('fr').t('errors.timeout')).toBe('The request timed out')
  })

  it('lets the app override single texts', () => {
    const i18n = createI18n('en', { 'common.confirm': 'Delete' })
    expect(i18n.t('common.confirm')).toBe('Delete')
    expect(i18n.t('common.cancel')).toBe('Cancel')
  })

  it('substitutes params and leaves unknown placeholders', () => {
    const i18n = createI18n('en', {
      'x.greet': 'Hi {name}, {count} new {missing}',
    })
    expect(i18n.t('x.greet', { name: 'Ada', count: 3 })).toBe(
      'Hi Ada, 3 new {missing}',
    )
  })

  it('shows the key itself when missing', () => {
    expect(createI18n('en').t('providers.unknown')).toBe('providers.unknown')
  })

  it('both locales define the same keys', () => {
    expect(Object.keys(zhCN).sort()).toEqual(Object.keys(en).sort())
  })
})
