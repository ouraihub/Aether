import {
  isContract,
  type ModelFilter,
  type ProviderSave,
  type EndpointKeySave,
  type ProbeTarget,
} from '@ouraihub/aether-contracts'
import { KitFailure, badRequest } from '@ouraihub/aether-core/error'
import type { WorkerContext } from '../context.js'
import type { ProviderService } from './index.js'

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export async function handleProviderRequest(
  service: ProviderService,
  request: Request,
  context: WorkerContext,
  authorizeReveal: (request: Request, context: WorkerContext) => Promise<void>,
): Promise<Response | null> {
  const url = new URL(request.url)
  const path = url.pathname
  const root = '/api/v1/providers'
  if (path !== root && !path.startsWith(`${root}/`)) return null
  const actor = context.actor
  const json = (value: unknown): Response => Response.json(value)
  if (path === root && request.method === 'GET') return json(await service.list(actor))
  if (path === root && request.method === 'POST') {
    const value: unknown = await request.json()
    if (!isContract('ProviderSave', value)) throw badRequest('provider', 'invalid')
    return json(await service.save(actor, value as ProviderSave))
  }
  if (path === `${root}/model-options` && request.method === 'GET') {
    const value: unknown = {
      ...(url.searchParams.has('modelType')
        ? { modelType: url.searchParams.get('modelType') }
        : {}),
      ...(url.searchParams.has('supportsTools')
        ? { supportsTools: url.searchParams.get('supportsTools') === 'true' }
        : {}),
    }
    if (
      url.searchParams.has('supportsTools') &&
      !['true', 'false'].includes(url.searchParams.get('supportsTools') ?? '')
    )
      throw badRequest('supportsTools', 'invalid')
    if (!isContract('ModelFilter', value)) throw badRequest('filter', 'invalid')
    return json(await service.modelOptions(actor, value as ModelFilter))
  }
  if (path === `${root}/probe` && request.method === 'POST') {
    const value: unknown = await request.json()
    if (!isContract('ProbeTarget', value)) throw badRequest('target', 'invalid')
    return json(await service.probe(actor, value as ProbeTarget, context.signal))
  }
  if (path === `${root}/keys` && request.method === 'POST') {
    const value: unknown = await request.json()
    if (!isContract('EndpointKeySave', value)) throw badRequest('key', 'invalid')
    return json(await service.saveKey(actor, value as EndpointKeySave))
  }
  const reveal = path.match(/^\/api\/v1\/providers\/keys\/([^/]+)\/reveal$/)
  if (reveal && request.method === 'POST') {
    await authorizeReveal(request, context)
    return json(await service.revealKey(actor, decodeURIComponent(reveal[1] ?? '')))
  }
  const key = path.match(/^\/api\/v1\/providers\/([^/]+)\/keys\/([^/]+)$/)
  if (key && request.method === 'DELETE')
    return json(
      await service.removeKey(
        actor,
        decodeURIComponent(key[1] ?? ''),
        decodeURIComponent(key[2] ?? ''),
      ),
    )
  const enabled = path.match(/^\/api\/v1\/providers\/([^/]+)\/enabled$/)
  if (enabled && request.method === 'POST') {
    const value: unknown = await request.json()
    if (!record(value) || Object.keys(value).length !== 1 || typeof value.enabled !== 'boolean')
      throw badRequest('enabled', 'invalid')
    return json(
      await service.setEnabled(actor, decodeURIComponent(enabled[1] ?? ''), value.enabled),
    )
  }
  const provider = path.match(/^\/api\/v1\/providers\/([^/]+)$/)
  if (provider && request.method === 'DELETE')
    return json(await service.remove(actor, decodeURIComponent(provider[1] ?? '')))
  throw new KitFailure('not_found', 'Unknown route', { entity: 'route' })
}
