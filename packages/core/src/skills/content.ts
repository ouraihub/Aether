export interface ContentIdentity {
  readonly name: string
  readonly description: string
  readonly version: string | null
  readonly body: string
  readonly license: string | null
}

export interface FileIdentity {
  readonly path: string
  readonly sha256: string
}

/** docs14 §3.1: validated text and unique files in ascending path order are prerequisites.
 * No trimming, newline normalization, sorting or case conversion is performed here.
 */
export async function digest(content: ContentIdentity, files: readonly FileIdentity[] = []): Promise<string> {
  const encoder = new TextEncoder()
  const fields: (string | null)[] = [content.name, content.description, content.version, content.body]
  if (content.license !== null || files.length !== 0) {
    fields.push('package', content.license)
    for (const file of files) fields.push(file.path, file.sha256)
  }
  const encoded = fields.map((field) => (field === null ? null : encoder.encode(field)))
  const bytes = new Uint8Array(encoded.reduce((size, field) => size + (field === null ? 1 : 9 + field.length), 0))
  const view = new DataView(bytes.buffer)
  let offset = 0
  for (const field of encoded) {
    bytes[offset++] = field === null ? 0 : 1
    if (field === null) continue
    view.setBigUint64(offset, BigInt(field.length), true)
    offset += 8
    bytes.set(field, offset)
    offset += field.length
  }
  return sha256(bytes)
}

export async function sha256(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))
  return Array.from(hash, (byte) => byte.toString(16).padStart(2, '0')).join('')
}
