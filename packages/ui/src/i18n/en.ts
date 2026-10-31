import type { KitErrorCode } from '../ports'

/** Every `errors.<code>` must exist; other keys are free-form and checked by
 * `pnpm kit:i18n`.
 */
export type Messages = Record<
  `errors.${KitErrorCode}` | 'common.confirm' | 'common.cancel',
  string
> & Record<string, string>

import { enPart1 } from './en-1'

import { enPart2 } from './en-2'

export const en: Messages = {
  ...enPart1,
  ...enPart2,
}
