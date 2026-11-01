import type { ConversationService } from './index.ts'
import { badRequest, KitFailure } from '@ouraihub/aether-core/error'
import type { Actor, WorkerContext } from '../context.ts'
import { isRecord } from '@ouraihub/aether-core/guard'

const notFound = (entity: string): KitFailure =>
  new KitFailure('not_found', 'Resource not found', { entity })

type ConversationRouteService = Pick<
  ConversationService,
  'list' | 'get' | 'delete' | 'rename' | 'editTurn' | 'removeTurn' | 'selectTurn' | 'fork'
>
type ImageResponse = (
  id: string,
  turnId: string,
  kind: 'input' | 'generated',
  index: number,
  actor: Actor,
) => Promise<Response>

export async function handleConversationRequest(
  service: ConversationRouteService,
  request: Request,
  context: WorkerContext,
  imageResponse: ImageResponse,
): Promise<Response | null> {
  const url = new URL(request.url),
    base = '/api/v1/conversations',
    actor = context.actor
  if (url.pathname !== base && !url.pathname.startsWith(`${base}/`)) return null
  const parts = url.pathname
    .slice(base.length)
    .split('/')
    .filter(Boolean)
    .map((part) => {
      try {
        return decodeURIComponent(part)
      } catch {
        throw badRequest('path', 'encoding')
      }
    })
  const json = (value: unknown): Response => Response.json(value)
  if (parts.length === 0 && request.method === 'GET')
    return json(await service.list(url.searchParams.get('scope'), actor))
  const id = parts[0]
  if (!id) throw notFound('route')
  if (parts.length === 1 && request.method === 'GET') return json(await service.get(id, actor))
  if (parts.length === 1 && request.method === 'DELETE') {
    await service.delete(id, actor)
    return json({ ok: true })
  }
  if (
    parts.length === 6 &&
    parts[1] === 'turns' &&
    parts[3] === 'images' &&
    request.method === 'GET'
  ) {
    const turnId = parts[2],
      kind = parts[4],
      index = parts[5]
    if (
      !turnId ||
      (kind !== 'input' && kind !== 'generated') ||
      !index ||
      !/^(0|[1-9]\d*)$/u.test(index)
    )
      throw notFound('image')
    return imageResponse(id, turnId, kind, Number(index), actor)
  }
  if (parts.length !== 2 || request.method !== 'POST') throw notFound('route')
  let body: unknown
  try {
    body = await request.json()
  } catch {
    throw badRequest('body', 'json')
  }
  if (!isRecord(body)) throw badRequest('body', 'object')
  const action = parts[1],
    fields =
      action === 'rename'
        ? ['title']
        : action === 'edit-turn'
          ? ['turnId', 'text', 'expectedText']
          : ['turnId']
  if (Object.keys(body).some((key) => !fields.includes(key)))
    throw badRequest('body', 'unknown_field')
  if (action === 'rename') {
    if (typeof body.title !== 'string') throw badRequest('title', 'required')
    return json(await service.rename(id, body.title, actor))
  }
  if (typeof body.turnId !== 'string') throw badRequest('turnId', 'required')
  if (action === 'edit-turn') {
    if (typeof body.text !== 'string' || typeof body.expectedText !== 'string')
      throw badRequest('text', 'required')
    return json(await service.editTurn(id, body.turnId, body.text, body.expectedText, actor))
  }
  if (action === 'remove-turn') return json(await service.removeTurn(id, body.turnId, actor))
  if (action === 'select-turn') return json(await service.selectTurn(id, body.turnId, actor))
  if (action === 'fork') return json(await service.fork(id, body.turnId, actor))
  throw notFound('route')
}
