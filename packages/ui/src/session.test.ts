import { expect, it } from 'vitest'
import { createClientSession } from './session'

it('accepts only the current token from the same session instance', () => {
  const first = createClientSession()
  const second = createClientSession()
  const token = first.capture()
  expect(first.isCurrent(token)).toBe(true)
  expect(second.capture().generation).toBe(token.generation)
  expect(second.isCurrent(token)).toBe(false)
  first.invalidate()
  expect(first.isCurrent(token)).toBe(false)
  expect(first.isCurrent(first.capture())).toBe(true)
})

it('never revives an old token after repeated invalidations', () => {
  const session = createClientSession()
  const tokens = [session.capture()]
  for (let index = 0; index < 3; index++) {
    session.invalidate()
    tokens.push(session.capture())
  }
  expect(tokens.slice(0, -1).some((token) => session.isCurrent(token))).toBe(false)
  expect(tokens.filter((token) => session.isCurrent(token))).toEqual([session.capture()])
})
