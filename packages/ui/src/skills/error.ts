// Error text for skill commands: the docs/14 reasons the user can act on, else the generic code text.
import { isKitClientError } from '../ports'
import type { KitI18n } from '../i18n'

export function errorText(t: KitI18n['t'], error: unknown): string {
  if (!isKitClientError(error)) return t('errors.internal')
  const { reason, field, limit } = error.details
  switch (reason) {
    case 'exists':
      return t('skills.err.exists')
    case 'builtin_name':
      return t('skills.err.builtin_name')
    case 'revision_changed':
      return t('skills.err.revision_changed')
    case 'secret_detected':
      return t('skills.err.secret_detected', { field: field ?? '' })
    case 'unsupported_files':
      return t('skills.err.unsupported_files', { field: field ?? '' })
    case 'too_large':
      return t('skills.err.too_large', { field: field ?? '', limit: limit ?? '' })
    case 'frontmatter_unsupported':
      return t('skills.err.frontmatter', { field: field ?? '' })
    default:
      return field ? `${t(`errors.${error.code}`)} (${field})` : t(`errors.${error.code}`)
  }
}
