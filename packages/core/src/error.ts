import type { Details, KitError, KitErrorCode } from '@ouraihub/aether-contracts'

export class KitFailure extends Error {
  readonly code: KitErrorCode
  readonly details: Details | undefined
  constructor(code: KitErrorCode, message: string, details?: Details) {
    super(message)
    this.name = 'KitFailure'
    this.code = code
    this.details = details
  }
  toJSON(): KitError {
    return this.details
      ? { code: this.code, message: this.message, details: this.details }
      : { code: this.code, message: this.message }
  }
}
export const badRequest = (field: string, reason: string, limit?: number): KitFailure =>
  new KitFailure('bad_request', `Invalid ${field}: ${reason}`, {
    field,
    reason,
    ...(limit === undefined ? {} : { limit }),
  })
export const cancelled = (): KitFailure => new KitFailure('cancelled', 'Cancelled')
export function safeError(error: unknown): KitError {
  return error instanceof KitFailure ? error.toJSON() : { code: 'internal', message: 'Internal execution failure' }
}
export function excerpt(body: string, secrets: readonly string[] = []): string {
  let text = body.replace(/\s+/gu, ' ')
  for (const secret of secrets) if (secret) text = text.replaceAll(secret, '***')
  text = text
    .replace(/\bBearer\s+\S+/gi, 'Bearer ***')
    .replace(/\b(?:sk-|ghp_|gho_|github_pat_|xoxb-|xoxp-|AKIA|AIza|glpat-)[A-Za-z0-9_./+=-]+/g, '***')
  return [...text].slice(0, 300).join('')
}
