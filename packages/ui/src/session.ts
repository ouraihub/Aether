/** A token belongs to exactly one session instance and one target generation. */
export interface SessionToken {
  readonly generation: number
}
export interface ClientSession {
  capture(): SessionToken
  isCurrent(token: SessionToken): boolean
  invalidate(): void
}
export function createClientSession(): ClientSession {
  let generation = 0
  let current: SessionToken = { generation }
  return {
    capture: () => current,
    isCurrent: (token) => token === current,
    invalidate: () => {
      current = { generation: ++generation }
    },
  }
}
