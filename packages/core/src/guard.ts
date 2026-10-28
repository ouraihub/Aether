export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
export function at(value: unknown, ...path: readonly (string | number)[]): unknown {
  let current = value
  for (const key of path) {
    if (Array.isArray(current) && typeof key === 'number') current = current[key]
    else if (isRecord(current)) current = current[String(key)]
    else return undefined
  }
  return current
}
export const charCount = (text: string): number => Array.from(text).length
export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`
  if (isRecord(value))
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(',')}}`
  const encoded = JSON.stringify(value)
  if (encoded === undefined) throw new TypeError('Not JSON')
  return encoded
}
export function truncate(text: string, limit: number, marker = '…(truncated)'): string {
  const chars = Array.from(text)
  if (chars.length <= limit) return text
  return chars.slice(0, Math.max(0, limit - charCount(marker))).join('') + marker.slice(0, limit)
}
export const tokens = (value: unknown): number | null =>
  typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff ? value : null
