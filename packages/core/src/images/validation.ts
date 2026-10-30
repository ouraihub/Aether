import { badRequest } from '../error.js'

const invalid = (): never => {
  throw badRequest('images', 'encoding')
}

/** Structural image validation shared by independent generation and host storage adapters. */
export function validateImageBytes(
  bytes: Uint8Array,
  expectedMime?: string,
  maxBytes = 4 * 1024 * 1024,
): { mime: string; width: number; height: number; alpha: boolean } {
  if (!bytes.length) return invalid()
  if (bytes.length > maxBytes) throw badRequest('images', 'size', maxBytes)
  const signature = (start: number, end: number): string => String.fromCharCode(...bytes.subarray(start, end))
  const mime =
    bytes[0] === 137 && signature(1, 4) === 'PNG'
      ? 'image/png'
      : bytes[0] === 255 && bytes[1] === 216
        ? 'image/jpeg'
        : signature(0, 4) === 'RIFF' && signature(8, 12) === 'WEBP'
          ? 'image/webp'
          : ['GIF87a', 'GIF89a'].includes(signature(0, 6))
            ? 'image/gif'
            : undefined
  if (!mime || (expectedMime !== undefined && expectedMime !== mime)) throw badRequest('images', 'mime')
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const tag = (offset: number, text: string): boolean => {
    for (let index = 0; index < text.length; index++) if (bytes[offset + index] !== text.charCodeAt(index)) return false
    return true
  }
  let width = 0,
    height = 0,
    alpha = false
  if (mime === 'image/png') {
    if (bytes.length < 57 || ![137, 80, 78, 71, 13, 10, 26, 10].every((v, i) => bytes[i] === v)) return invalid()
    let offset = 8,
      first = true,
      data = false,
      end = false
    while (offset + 12 <= bytes.length) {
      const size = view.getUint32(offset)
      if (size > bytes.length - offset - 12) return invalid()
      const kind = offset + 4
      if (first) {
        if (!tag(kind, 'IHDR') || size !== 13) return invalid()
        width = view.getUint32(offset + 8)
        height = view.getUint32(offset + 12)
        const depth = bytes[offset + 16],
          color = bytes[offset + 17]
        if (
          depth === undefined ||
          color === undefined ||
          ![0, 2, 3, 4, 6].includes(color) ||
          !(color === 0 ? [1, 2, 4, 8, 16] : color === 3 ? [1, 2, 4, 8] : [8, 16]).includes(depth) ||
          bytes[offset + 18] !== 0 ||
          bytes[offset + 19] !== 0 ||
          ![0, 1].includes(bytes[offset + 20] ?? -1)
        )
          return invalid()
        alpha = color === 4 || color === 6
        first = false
      } else if (tag(kind, 'IHDR')) return invalid()
      if (tag(kind, 'tRNS')) alpha = true
      if (tag(kind, 'IDAT') && size > 0) data = true
      offset += size + 12
      if (tag(kind, 'IEND')) {
        if (size !== 0 || offset !== bytes.length) return invalid()
        end = true
        break
      }
    }
    if (!data || !end) return invalid()
  } else if (mime === 'image/jpeg') {
    if (bytes.length < 12 || bytes[0] !== 255 || bytes[1] !== 216) return invalid()
    let offset = 2,
      scan = false,
      end = false
    while (offset < bytes.length) {
      if (bytes[offset++] !== 255) return invalid()
      while (bytes[offset] === 255) offset++
      const marker = bytes[offset++]
      if (marker === 217) {
        end = offset === bytes.length
        break
      }
      if (marker === undefined || marker === 0 || marker === 216 || offset + 2 > bytes.length) return invalid()
      if (marker >= 208 && marker <= 215) continue
      const size = view.getUint16(offset)
      if (size < 2 || offset + size > bytes.length) return invalid()
      if ([192, 193, 194, 195, 197, 198, 199, 201, 202, 203, 205, 206, 207].includes(marker)) {
        if (size < 8 || width !== 0) return invalid()
        height = view.getUint16(offset + 3)
        width = view.getUint16(offset + 5)
      }
      offset += size
      if (marker === 218) {
        scan = true
        while (offset < bytes.length) {
          if (bytes[offset] !== 255) {
            offset++
            continue
          }
          if (bytes[offset + 1] === 0 || ((bytes[offset + 1] ?? 0) >= 208 && (bytes[offset + 1] ?? 0) <= 215)) {
            offset += 2
            continue
          }
          break
        }
      }
    }
    if (!scan || !end) return invalid()
  } else if (mime === 'image/webp') {
    if (bytes.length < 26 || !tag(0, 'RIFF') || !tag(8, 'WEBP') || view.getUint32(4, true) + 8 !== bytes.length)
      return invalid()
    let offset = 12,
      image = false,
      extended = false,
      animated = false,
      animationHeader = false
    while (offset + 8 <= bytes.length) {
      const size = view.getUint32(offset + 4, true),
        body = offset + 8
      if (size > bytes.length - body) return invalid()
      if (tag(offset, 'VP8X')) {
        if (offset !== 12 || size !== 10) return invalid()
        extended = true
        alpha = ((bytes[body] ?? 0) & 16) !== 0
        animated = ((bytes[body] ?? 0) & 2) !== 0
        width = 1 + (bytes[body + 4] ?? 0) + ((bytes[body + 5] ?? 0) << 8) + ((bytes[body + 6] ?? 0) << 16)
        height = 1 + (bytes[body + 7] ?? 0) + ((bytes[body + 8] ?? 0) << 8) + ((bytes[body + 9] ?? 0) << 16)
      } else if (tag(offset, 'ANIM')) {
        if (!animated || size !== 6 || animationHeader) return invalid()
        animationHeader = true
      } else if (tag(offset, 'ANMF')) {
        if (!animated || !animationHeader || size < 24) return invalid()
        const u24 = (at: number): number =>
          (bytes[at] ?? 0) + ((bytes[at + 1] ?? 0) << 8) + ((bytes[at + 2] ?? 0) << 16)
        const x = u24(body) * 2,
          y = u24(body + 3) * 2,
          w = u24(body + 6) + 1,
          h = u24(body + 9) + 1
        if (x + w > width || y + h > height || ((bytes[body + 15] ?? 0) & 252) !== 0) return invalid()
        let frame = body + 16,
          pixels = false
        while (frame + 8 <= body + size) {
          const length = view.getUint32(frame + 4, true),
            data = frame + 8
          if (data + length > body + size) return invalid()
          if (tag(frame, 'VP8 ')) {
            if (
              pixels ||
              length < 10 ||
              ((bytes[data] ?? 1) & 1) !== 0 ||
              !tag(data + 3, '\u009d\u0001\u002a') ||
              (view.getUint16(data + 6, true) & 16383) !== w ||
              (view.getUint16(data + 8, true) & 16383) !== h
            )
              return invalid()
            pixels = true
          } else if (tag(frame, 'VP8L')) {
            if (pixels || length < 5 || bytes[data] !== 47) return invalid()
            const bits = view.getUint32(data + 1, true)
            if (bits >>> 29 !== 0 || (bits & 16383) + 1 !== w || ((bits >>> 14) & 16383) + 1 !== h) return invalid()
            alpha ||= ((bits >>> 28) & 1) !== 0
            pixels = true
          } else if (!tag(frame, 'ALPH')) return invalid()
          frame = data + length + (length & 1)
        }
        if (!pixels || frame !== body + size) return invalid()
        image = true
      } else if (tag(offset, 'VP8 ') || tag(offset, 'VP8L')) {
        if (animated) return invalid()
        if (image) return invalid()
        image = true
        let w: number, h: number
        if (tag(offset, 'VP8 ')) {
          if (size < 10 || ((bytes[body] ?? 1) & 1) !== 0 || !tag(body + 3, '\u009d\u0001\u002a')) return invalid()
          w = view.getUint16(body + 6, true) & 16383
          h = view.getUint16(body + 8, true) & 16383
        } else {
          if (size < 5 || bytes[body] !== 47) return invalid()
          const bits = view.getUint32(body + 1, true)
          if (bits >>> 29 !== 0) return invalid()
          w = (bits & 16383) + 1
          h = ((bits >>> 14) & 16383) + 1
          alpha ||= ((bits >>> 28) & 1) !== 0
        }
        if (extended && (w !== width || h !== height)) return invalid()
        width = w
        height = h
      }
      offset = body + size + (size & 1)
    }
    if (!image || offset !== bytes.length) return invalid()
  } else if (mime === 'image/gif') {
    if (bytes.length < 14 || (!tag(0, 'GIF87a') && !tag(0, 'GIF89a'))) return invalid()
    width = view.getUint16(6, true)
    height = view.getUint16(8, true)
    let offset = 13
    const packed = bytes[10] ?? 0
    if (packed & 128) offset += 3 * (1 << ((packed & 7) + 1))
    const blocks = (): void => {
      while (true) {
        if (offset >= bytes.length) return invalid()
        const size = bytes[offset++] ?? 0
        if (!size) return
        if (size > bytes.length - offset) return invalid()
        offset += size
      }
    }
    let frames = 0,
      end = false
    while (offset < bytes.length) {
      const marker = bytes[offset++]
      if (marker === 59) {
        end = offset === bytes.length
        break
      }
      if (marker === 33) {
        if (offset >= bytes.length) return invalid()
        const label = bytes[offset++]
        if (label === 249) {
          if (bytes[offset] !== 4 || offset + 6 > bytes.length) return invalid()
          alpha ||= ((bytes[offset + 1] ?? 0) & 1) !== 0
        }
        blocks()
      } else if (marker === 44) {
        if (offset + 9 > bytes.length) return invalid()
        const x = view.getUint16(offset, true),
          y = view.getUint16(offset + 2, true)
        const w = view.getUint16(offset + 4, true),
          h = view.getUint16(offset + 6, true)
        if (!w || !h || x + w > width || y + h > height) return invalid()
        const flags = bytes[offset + 8] ?? 0
        offset += 9
        if (flags & 128) offset += 3 * (1 << ((flags & 7) + 1))
        const codeSize = bytes[offset++]
        if (codeSize === undefined || codeSize < 2 || codeSize > 8 || !bytes[offset]) return invalid()
        blocks()
        frames++
      } else return invalid()
    }
    if (!frames || !end) return invalid()
  } else return invalid()
  if (width < 1 || height < 1 || width * height > 40_000_000) throw badRequest('images', 'dimensions')
  return { mime, width, height, alpha }
}
