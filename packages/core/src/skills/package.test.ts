import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { deflateRawSync } from 'node:zlib'
import { readSkillSource } from './package.js'
import * as skills from './service.js'

const encoder = new TextEncoder()
const markdown = '---\nname: packed\ndescription: package\n---\nbody'
function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff
  for (const byte of bytes) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  return (crc ^ 0xffffffff) >>> 0
}
/** Deterministic in-memory ZIP fixture; no filesystem operations. */
function zip(entries: readonly { path: string; body: string; mode?: number; flags?: number; deflate?: boolean }[]) {
  const chunks: Uint8Array[] = []
  const central: Uint8Array[] = []
  let offset = 0
  for (const entry of entries) {
    const name = encoder.encode(entry.path)
    const body = encoder.encode(entry.body)
    const compressed = entry.deflate ? deflateRawSync(body) : body
    const local = new Uint8Array(30 + name.length + compressed.length)
    const view = new DataView(local.buffer)
    view.setUint32(0, 0x04034b50, true)
    view.setUint16(4, 20, true)
    view.setUint16(6, entry.flags ?? 0, true)
    view.setUint16(8, entry.deflate ? 8 : 0, true)
    view.setUint32(14, crc32(body), true)
    view.setUint32(18, compressed.length, true)
    view.setUint32(22, body.length, true)
    view.setUint16(26, name.length, true)
    local.set(name, 30)
    local.set(compressed, 30 + name.length)
    const header = new Uint8Array(46 + name.length)
    const cv = new DataView(header.buffer)
    cv.setUint32(0, 0x02014b50, true)
    cv.setUint16(4, 0x0314, true)
    cv.setUint16(6, 20, true)
    cv.setUint16(8, entry.flags ?? 0, true)
    cv.setUint16(10, entry.deflate ? 8 : 0, true)
    cv.setUint32(16, crc32(body), true)
    cv.setUint32(20, compressed.length, true)
    cv.setUint32(24, body.length, true)
    cv.setUint16(28, name.length, true)
    cv.setUint32(38, (entry.mode ?? 0x81a4) * 65536, true)
    cv.setUint32(42, offset, true)
    header.set(name, 46)
    chunks.push(local)
    central.push(header)
    offset += local.length
  }
  const end = new Uint8Array(22)
  const ev = new DataView(end.buffer)
  ev.setUint32(0, 0x06054b50, true)
  ev.setUint16(8, entries.length, true)
  ev.setUint16(10, entries.length, true)
  ev.setUint32(
    12,
    central.reduce((size, bytes) => size + bytes.length, 0),
    true,
  )
  ev.setUint32(16, offset, true)
  const all = [...chunks, ...central, end]
  const bytes = new Uint8Array(all.reduce((size, chunk) => size + chunk.length, 0))
  let position = 0
  for (const chunk of all) {
    bytes.set(chunk, position)
    position += chunk.length
  }
  return { bytes: { fileName: 'skill.zip', bytes: Array.from(bytes) } }
}

describe('bounded ZIP skill packages', () => {
  it('reads deflated files and caps actual inflated bytes', async () => {
    const content = await readSkillSource(zip([{ path: 'SKILL.md', body: markdown, deflate: true }]))
    assert.equal(content.name, 'packed')
    await assert.rejects(
      readSkillSource(
        zip([
          { path: 'SKILL.md', body: markdown },
          { path: 'assets/bomb', body: 'x'.repeat(1024 * 1024 + 1), deflate: true },
        ]),
      ),
      { details: { field: 'assets/bomb', reason: 'too_large', limit: 1024 * 1024 } },
    )
  })
  it('retains sorted scripts/assets/license across install/manual edit/duplicate and pinned reads', async () => {
    const content = await readSkillSource(
      zip([
        { path: 'wrapper/SKILL.md', body: markdown },
        { path: 'wrapper/LICENSE', body: 'MIT' },
        { path: 'wrapper/scripts/z.sh', body: 'echo safe' },
        { path: 'wrapper/assets/a.txt', body: 'asset' },
      ]),
    )
    assert.deepEqual(
      content.files.map((file) => [file.path, file.kind]),
      [
        ['assets/a.txt', 'asset'],
        ['scripts/z.sh', 'script'],
      ],
    )
    const preview = await skills.preview([], content)
    let catalog = await skills.install([], content, preview.digest, false, 'one', 1)
    catalog = await skills.save(
      catalog,
      { id: 'one', expectedRevision: 1, name: 'packed', description: 'package', body: 'edited' },
      '',
      2,
    )
    catalog = await skills.duplicate(catalog, 'one', 'copy', 'two', 3)
    assert.equal(skills.get(catalog, 'two').license, 'MIT')
    assert.equal(skills.get(catalog, 'two').hasScripts, true)
    assert.equal(new TextDecoder().decode(skills.file(catalog, 'two', 1, 'scripts/z.sh')), 'echo safe')
    assert.throws(() => skills.files(catalog, 'one', 1))
    assert.throws(() => skills.file(catalog, 'two', 1, 'missing'))
    const changed = { ...content, files: content.files.filter((file) => file.kind !== 'script') }
    const changedPreview = await skills.preview(catalog, changed)
    catalog = await skills.install(catalog, changed, changedPreview.digest, true, '', 4)
    assert.equal(skills.get(catalog, 'one').revision, 3)
  })
  it('rejects unsafe paths, duplicates, links, encryption, secrets, depth and size violations', async () => {
    for (const entry of [
      { path: '../SKILL.md', reason: 'path_traversal' },
      { path: '/SKILL.md', reason: 'absolute_path' },
      { path: 'a\\SKILL.md', reason: 'backslash_path' },
    ]) {
      await assert.rejects(readSkillSource(zip([{ path: entry.path, body: markdown }])), {
        details: { field: 'zip', reason: entry.reason },
      })
    }
    await assert.rejects(
      readSkillSource(
        zip([
          { path: 'SKILL.md', body: markdown },
          { path: './SKILL.md', body: markdown },
        ]),
      ),
      { details: { field: 'zip', reason: 'duplicate_path' } },
    )
    await assert.rejects(readSkillSource(zip([{ path: 'SKILL.md', body: markdown, mode: 0xa1ff }])), {
      details: { field: 'zip', reason: 'link' },
    })
    await assert.rejects(readSkillSource(zip([{ path: 'SKILL.md', body: markdown, flags: 1 }])), {
      details: { field: 'zip', reason: 'encrypted' },
    })
    await assert.rejects(
      readSkillSource(
        zip([
          { path: 'SKILL.md', body: markdown },
          { path: 'assets/a.txt', body: 'ghp_abcdefghijklmnopqrstuvwxyz' },
        ]),
      ),
      { details: { field: 'assets/a.txt:1', reason: 'secret_detected' } },
    )
    await assert.rejects(
      readSkillSource(
        zip([
          { path: 'SKILL.md', body: markdown },
          { path: 'scripts/a/b/c/d/x.sh', body: 'x' },
        ]),
      ),
      { details: { field: 'scripts/a/b/c/d/x.sh', reason: 'too_deep', limit: 4 } },
    )
    await assert.rejects(
      readSkillSource(
        zip([
          { path: 'SKILL.md', body: markdown },
          { path: 'run.sh', body: 'x' },
        ]),
      ),
      { details: { field: 'run.sh', reason: 'unsupported_files' } },
    )
    await assert.rejects(
      readSkillSource(
        zip([
          { path: 'SKILL.md', body: markdown },
          { path: 'assets/large', body: 'x'.repeat(1024 * 1024 + 1) },
        ]),
      ),
      { details: { field: 'assets/large', reason: 'too_large', limit: 1024 * 1024 } },
    )
  })
})
