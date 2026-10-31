import type { HistoryTurn, Message } from './types.js'
import { charCount } from './guard.js'

/** Estimates text only: images are neither token-estimated nor copied. */
export function messageChars(message: Message): number {
  if ('user' in message) return charCount(message.user)
  if ('tool' in message) return charCount(message.tool.text)
  return (
    charCount(message.assistant.text) +
    charCount(JSON.stringify(message.assistant.tool_calls)) +
    charCount(JSON.stringify(message.assistant.reasoning ?? []))
  )
}
/** Drops complete oldest turns; never breaks tool-call/result pairing. */
export function trimHistory(
  turns: readonly HistoryTurn[],
  availableTokens: number,
  canVision: boolean,
): Message[] {
  const groups = turns
    .filter((turn) => turn.status === 'completed')
    .map((turn): Message[] => [
      {
        user: turn.input,
        ...(canVision && turn.images?.length ? { images: turn.images } : {}),
      },
      ...turn.messages,
    ])
  let size = groups.reduce(
    (total, group) =>
      total + group.reduce((sum, message) => sum + messageChars(message), 0),
    0,
  )
  let start = 0
  while (
    Math.ceil(size / 3) > Math.max(0, availableTokens) &&
    start < groups.length
  ) {
    const group = groups[start]
    if (!group) break
    size -= group.reduce((sum, message) => sum + messageChars(message), 0)
    start++
  }
  return groups.slice(start).flat()
}
