import test from 'node:test'
import assert from 'node:assert/strict'
import { validateArguments } from '../src/schema.js'

test('Draft07 tools validate local refs, numeric bounds and exact object properties', () => {
  const schema = {
    type: 'object',
    definitions: { positive: { type: 'integer', minimum: 1, maximum: 8 } },
    properties: {
      count: { $ref: '#/definitions/positive' },
      names: {
        type: 'array',
        minItems: 1,
        uniqueItems: true,
        items: { type: 'string', minLength: 2, pattern: '^[a-z]+$' },
      },
    },
    required: ['count', 'names'],
    additionalProperties: false,
  }
  assert.equal(
    validateArguments(schema, { count: 2, names: ['one', 'two'] }),
    true,
  )
  for (const args of [
    { count: 0, names: ['one'] },
    { count: 2, names: ['a'] },
    { count: 2, names: ['one', 'one'] },
    { count: 2, names: ['ONE'] },
    { count: 2, names: ['one'], extra: true },
    { names: ['one'] },
  ])
    assert.equal(validateArguments(schema, args), false)
})
test('combinators, conditionals, tuple items, dependencies and formats are enforced', () => {
  const schema = {
    type: 'object',
    properties: {
      mode: { enum: ['read', 'write'] },
      target: { type: 'string' },
      amount: { type: 'number', multipleOf: 0.5 },
      tuple: {
        type: 'array',
        items: [{ type: 'string' }, { type: 'integer' }],
        additionalItems: false,
      },
      email: { type: 'string', format: 'email' },
    },
    required: ['mode'],
    if: { properties: { mode: { const: 'write' } } },
    then: { required: ['target'] },
    dependencies: { amount: ['target'] },
    additionalProperties: false,
  }
  assert.equal(
    validateArguments(schema, {
      mode: 'read',
      tuple: ['x', 1],
      email: 'x@example.com',
    }),
    true,
  )
  assert.equal(validateArguments(schema, { mode: 'write' }), false)
  assert.equal(validateArguments(schema, { mode: 'read', amount: 1 }), false)
  assert.equal(
    validateArguments(schema, { mode: 'read', email: 'invalid' }),
    false,
  )
  assert.equal(
    validateArguments(schema, { mode: 'read', tuple: ['x', 1, 2] }),
    false,
  )
  assert.equal(
    validateArguments(
      {
        type: 'object',
        properties: {
          value: {
            oneOf: [{ type: 'string' }, { type: 'number', minimum: 3 }],
          },
        },
        required: ['value'],
      },
      { value: 1 },
    ),
    false,
  )
})
test('untrusted schemas fail closed for unsupported semantics and bounded resource abuse', () => {
  for (const schema of [
    { type: 'object', unknownConstraint: true },
    { type: 'object', $ref: 'https://remote.example/schema' },
    { type: 'object', $ref: '#' },
    {
      type: 'object',
      properties: { value: { type: 'string', pattern: '(a+)+$' } },
    },
    {
      type: 'object',
      properties: {
        value: { type: 'string', format: 'invented-security-format' },
      },
    },
    { type: 'object', required: 'value' },
  ])
    assert.equal(validateArguments(schema, { value: 'x' }), false)
  assert.equal(
    validateArguments({ type: 'object' }, { value: 'x'.repeat(262145) }),
    false,
  )
  const cyclic: Record<string, unknown> = {}
  cyclic.self = cyclic
  assert.equal(validateArguments({ type: 'object' }, cyclic), false)
})
