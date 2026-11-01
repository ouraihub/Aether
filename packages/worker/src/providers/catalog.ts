import { isContract, type Provider } from '@ouraihub/aether-contracts'
import { KitFailure } from '@ouraihub/aether-core/error'
import { inferReasoning } from '@ouraihub/aether-core/providers/infer'
import { isSealedCredential, type SealedCredential } from '../credentials.js'

export interface Secret {
  endpointId: string
  keyId: string
  sealed: SealedCredential
}

export interface Catalog {
  providers: Provider[]
  secrets: Secret[]
  inferred: string[]
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function parseCatalog(value: unknown): Catalog {
  if (
    !record(value) ||
    !Array.isArray(value.providers) ||
    !Array.isArray(value.secrets) ||
    !Array.isArray(value.inferred)
  )
    throw new KitFailure('incompatible_db', 'Invalid provider catalog')
  const providers: Provider[] = []
  const secrets: Secret[] = []
  const inferred: string[] = []
  for (const item of value.providers) {
    if (!isContract('Provider', item))
      throw new KitFailure('incompatible_db', 'Invalid provider document')
    providers.push(item)
  }
  for (const item of value.secrets) {
    if (
      !record(item) ||
      typeof item.endpointId !== 'string' ||
      typeof item.keyId !== 'string' ||
      !isSealedCredential(item.sealed)
    )
      throw new KitFailure('incompatible_db', 'Invalid provider credential')
    secrets.push({ endpointId: item.endpointId, keyId: item.keyId, sealed: item.sealed })
  }
  for (const item of value.inferred) {
    if (typeof item !== 'string')
      throw new KitFailure('incompatible_db', 'Invalid reasoning declaration')
    inferred.push(item)
  }
  const automatic = new Set(inferred)
  for (const provider of providers)
    for (const endpoint of provider.endpoints)
      for (const model of endpoint.models)
        if (automatic.has(JSON.stringify([endpoint.id, model.id])))
          model.reasoning =
            model.modelType === 'chat'
              ? inferReasoning(model.id, endpoint.protocol, endpoint.baseUrl, model.maxOutput)
              : null
  return { providers, secrets, inferred }
}
