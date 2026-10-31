import { Validator, format } from '@cfworker/json-schema'
import type { Schema } from '@cfworker/json-schema'
import { isRecord } from './guard.js'
import type { ArgumentsValidator } from './ports.js'

const KEYWORDS = [
  '\u0024schema',
  '$id',
  '$ref',
  '$comment',
  '$defs',
  'definitions',
  'title',
  'description',
  'default',
  'examples',
  'readOnly',
  'writeOnly',
  'type',
  'enum',
  'const',
  'allOf',
  'anyOf',
  'oneOf',
  'not',
  'if',
  'then',
  'else',
  'minimum',
  'maximum',
  'exclusiveMinimum',
  'exclusiveMaximum',
  'multipleOf',
  'minLength',
  'maxLength',
  'pattern',
  'format',
  'contentEncoding',
  'contentMediaType',
  'minItems',
  'maxItems',
  'uniqueItems',
  'items',
  'additionalItems',
  'contains',
  'minProperties',
  'maxProperties',
  'required',
  'properties',
  'patternProperties',
  'additionalProperties',
  'dependencies',
  'propertyNames',
]
const TYPES = [
  'null',
  'boolean',
  'object',
  'array',
  'number',
  'integer',
  'string',
]
/** Complete Draft-07 validation through a CSP-safe interpreter. Resource and local-ref checks
 * precede interpretation. Unsupported extensions and unsafe patterns fail closed.
 */
export const validateArguments: ArgumentsValidator = (root, args) => {
  let nodes = 0,
    refs = 0
  const active = new Set<unknown>()
  const json = (value: unknown, depth: number): boolean => {
    if (++nodes > 20000 || depth > 64) return false
    if (
      value === null ||
      typeof value === 'string' ||
      typeof value === 'boolean'
    )
      return true
    if (typeof value === 'number') return Number.isFinite(value)
    return Array.isArray(value)
      ? value.every((item: unknown) => json(item, depth + 1))
      : isRecord(value) &&
          Object.values(value).every((item) => json(item, depth + 1))
  }
  const regex = (value: unknown): boolean => {
    if (
      typeof value !== 'string' ||
      value.length > 256 ||
      /\\[1-9]|\(\?|\([^)]*[+*{][^)]*\)[+*{]|(?:\.\*|\.\+).*(?:\.\*|\.\+)/.test(
        value,
      )
    )
      return false
    try {
      new RegExp(value, 'u')
      return true
    } catch {
      return false
    }
  }
  const resolve = (ref: string): unknown => {
    if (++refs > 128) return undefined
    if (ref === '#') return root
    if (!ref.startsWith('#/')) return undefined
    let value: unknown = root
    try {
      for (const part of decodeURIComponent(ref.slice(2)).split('/')) {
        const key = part.replaceAll('~1', '/').replaceAll('~0', '~')
        if (!isRecord(value) || !Object.hasOwn(value, key)) return undefined
        value = value[key]
      }
    } catch {
      return undefined
    }
    return value
  }
  const inspect = (schema: unknown, depth: number): boolean => {
    if (++nodes > 20000 || depth > 64) return false
    if (typeof schema === 'boolean') return true
    if (!isRecord(schema) || active.has(schema)) return false
    active.add(schema)
    try {
      for (const key of Object.keys(schema))
        if (!KEYWORDS.includes(key)) return false
      if (
        schema.$schema !== undefined &&
        schema.$schema !== 'http://json-schema.org/draft-07/schema#' &&
        schema.$schema !== 'https://json-schema.org/draft-07/schema'
      )
        return false
      if (schema.$id !== undefined && typeof schema.$id !== 'string')
        return false
      if (
        schema.type !== undefined &&
        !(typeof schema.type === 'string'
          ? TYPES.includes(schema.type)
          : Array.isArray(schema.type) &&
            schema.type.length > 0 &&
            schema.type.every(
              (type: unknown) =>
                typeof type === 'string' && TYPES.includes(type),
            ))
      )
        return false
      for (const key of [
        'minLength',
        'maxLength',
        'minItems',
        'maxItems',
        'minProperties',
        'maxProperties',
      ])
        if (
          schema[key] !== undefined &&
          !(
            typeof schema[key] === 'number' &&
            Number.isSafeInteger(schema[key]) &&
            schema[key] >= 0
          )
        )
          return false
      for (const key of [
        'minimum',
        'maximum',
        'exclusiveMinimum',
        'exclusiveMaximum',
        'multipleOf',
      ])
        if (
          schema[key] !== undefined &&
          !(
            typeof schema[key] === 'number' &&
            Number.isFinite(schema[key]) &&
            (key !== 'multipleOf' || schema[key] > 0)
          )
        )
          return false
      if (
        schema.required !== undefined &&
        !(
          Array.isArray(schema.required) &&
          schema.required.every((key: unknown) => typeof key === 'string') &&
          new Set(schema.required).size === schema.required.length
        )
      )
        return false
      if (
        schema.enum !== undefined &&
        !(Array.isArray(schema.enum) && schema.enum.length > 0)
      )
        return false
      if (
        schema.uniqueItems !== undefined &&
        typeof schema.uniqueItems !== 'boolean'
      )
        return false
      if (schema.pattern !== undefined && !regex(schema.pattern)) return false
      if (
        schema.format !== undefined &&
        (typeof schema.format !== 'string' ||
          !Object.hasOwn(format, schema.format))
      )
        return false
      if (
        schema.contentEncoding !== undefined ||
        schema.contentMediaType !== undefined
      )
        return false
      if (
        schema.$ref !== undefined &&
        (typeof schema.$ref !== 'string' ||
          !inspect(resolve(schema.$ref), depth + 1))
      )
        return false
      for (const key of ['allOf', 'anyOf', 'oneOf'])
        if (
          schema[key] !== undefined &&
          !(
            Array.isArray(schema[key]) &&
            schema[key].length > 0 &&
            schema[key].every((child: unknown) => inspect(child, depth + 1))
          )
        )
          return false
      for (const key of [
        'not',
        'if',
        'then',
        'else',
        'additionalItems',
        'contains',
        'additionalProperties',
        'propertyNames',
      ])
        if (schema[key] !== undefined && !inspect(schema[key], depth + 1))
          return false
      if (
        schema.items !== undefined &&
        !(Array.isArray(schema.items)
          ? schema.items.every((child: unknown) => inspect(child, depth + 1))
          : inspect(schema.items, depth + 1))
      )
        return false
      for (const key of [
        'properties',
        'patternProperties',
        'definitions',
        '$defs',
      ]) {
        const group = schema[key]
        if (group === undefined) continue
        if (!isRecord(group)) return false
        for (const [name, child] of Object.entries(group))
          if (
            (key === 'patternProperties' && !regex(name)) ||
            !inspect(child, depth + 1)
          )
            return false
      }
      if (schema.dependencies !== undefined) {
        if (!isRecord(schema.dependencies)) return false
        for (const child of Object.values(schema.dependencies))
          if (
            !(Array.isArray(child)
              ? child.every((item: unknown) => typeof item === 'string')
              : inspect(child, depth + 1))
          )
            return false
      }
      return true
    } finally {
      active.delete(schema)
    }
  }
  try {
    if (
      JSON.stringify(root).length > 32768 ||
      JSON.stringify(args).length > 262144 ||
      !json(root, 0) ||
      !json(args, 0) ||
      !inspect(root, 0)
    )
      return false
    const cloned: unknown = JSON.parse(JSON.stringify(root))
    if (!isRecord(cloned)) return false
    const schema: Schema = cloned
    return new Validator(schema, '7', true).validate(args).valid
  } catch {
    return false
  }
}
