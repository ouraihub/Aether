// Kit messages (docs/16 §5). `pnpm kit:i18n` checks both locales share one key
// set and every used key exists.
import { en } from './en'
import { zhCN } from './zh-CN'

export type { Messages } from './en'

export interface KitI18n {
  readonly locale: string
  t(key: string, params?: Record<string, string | number>): string
}

/** `zh*` locales get Chinese, everything else English; `overrides` replace kit
 * texts per key.
 */
export function createI18n(
  locale: string,
  overrides: Readonly<Record<string, string>> = {},
): KitI18n {
  const messages: Readonly<Record<string, string>> = locale
    .toLowerCase()
    .startsWith('zh')
    ? zhCN
    : en
  return {
    locale,
    t(key, params = {}) {
      // A missing key renders as itself so it is visible instead of blank.
      const text = overrides[key] ?? messages[key] ?? key
      return text.replace(/\{(\w+)\}/g, (match, name: string) =>
        String(params[name] ?? match),
      )
    },
  }
}
