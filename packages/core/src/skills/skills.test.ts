import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { digest } from './content.js'
import { readSkillSource } from './package.js'
import * as skills from './service.js'
import { parseManualSave, parseSkillMarkdown } from './validation.js'

const identity = { name: 'guide', description: 'test skill', version: null, body: 'body', license: null }
const source = (body: string) => ({
  bytes: { fileName: 'SKILL.md', bytes: Array.from(new TextEncoder().encode(body)) },
})
const markdown = '---\nname: guide\ndescription: test skill\n---\nbody'

describe('docs14 content identity', () => {
  it('matches Rust fixed vectors including presence, UTF8 and conditional package suffix', async () => {
    assert.equal(await digest(identity), 'a738f7fd408226db7f39ffb331ade74d1095d569a343ea9ce666f074351573cb')
    assert.equal(
      await digest({ ...identity, version: '' }),
      '06407a8e82b49c73b6adf3f212420b8e0077c95883fa8f085824ebf8c2666a59',
    )
    assert.equal(
      await digest({ ...identity, license: '' }),
      'fe3469f99f29a254a6189f75e5c62c8d2b3813b10dbbe5fae054c7c60c1b1883',
    )
    assert.equal(
      await digest({ ...identity, license: 'MIT' }),
      'cfdfc1aa6248e2e40e3d68d436bda2531eb9572fe0fa09f6dede0ac490ab5047',
    )
    const files = [
      { path: 'assets/a.bin', sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855' },
      { path: 'scripts/z.sh', sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad' },
    ]
    assert.equal(await digest(identity, files), '42d4089aa899ca6909e2ce724a633a78ca73bbe26724409adb4ba20b32f69c7c')
    assert.equal(
      await digest({ name: 'guide', description: '出行🚆', version: '版一', body: '正文\r\né', license: '许可' }, [
        { path: 'references/a.md', sha256: files[1]?.sha256 ?? '' },
      ]),
      '1440f8fa80ce9d82fae4c910d2ae1465a7b3e706555e6bb64da2f133302bebea',
    )
    assert.equal(
      await digest({ ...identity, body: ' body\r\n' }),
      '00b766b13756e19a9fd3e8e6d83d257715e4910b71adafcac4f955b19617f89a',
    )
  })
})

describe('skill package and management', () => {
  it('normalizes only markdown boundary and rejects unsupported syntax and secrets', async () => {
    assert.equal(parseSkillMarkdown(`\uFEFF${markdown.replaceAll('\n', '\r\n')}`).body, 'body')
    for (const line of [
      'description: [nested]',
      'description: &anchor',
      'description: |',
      'name: again',
      ' nested: no',
    ]) {
      assert.throws(() => parseSkillMarkdown(markdown.replace('---\nbody', `${line}\n---\nbody`)))
    }
    await assert.rejects(readSkillSource(source(`${markdown}\nAuthorization: Bearer abcdefghijklmnopqrstuvwxyz`)), {
      details: { reason: 'secret_detected', field: 'SKILL.md:6' },
    })
    assert.throws(() => parseManualSave({ name: 'guide', description: 'test', body: 'sk-abcdefghijklmnopqrstuvwxyz' }))
    assert.throws(() => parseManualSave({ name: 'guide', description: 'test', body: 'body', surprise: true }))
    await assert.rejects(readSkillSource({ path: '/etc/skill' }), {
      details: { field: 'source', reason: 'unsupported_source' },
    })
    await assert.rejects(readSkillSource({ bytes: { fileName: 'SKILL.md', bytes: [256] } }), { code: 'bad_request' })
  })
  it('guards confirmation/revision/name/builtins and exact reorder membership', async () => {
    const content = await readSkillSource(source(markdown))
    const preview = await skills.preview([], content)
    let catalog = await skills.install([], content, preview.digest, false, 'one', 10)
    await assert.rejects(skills.install(catalog, { ...content, body: 'changed' }, preview.digest, true, '', 20), {
      details: { reason: 'content_changed' },
    })
    assert.equal(skills.get(catalog, 'one').body, 'body')
    catalog = await skills.install(catalog, content, preview.digest, true, '', 20)
    assert.equal(skills.get(catalog, 'one').revision, 1)
    await assert.rejects(
      skills.save(catalog, { id: 'one', name: 'guide', description: 'test', body: 'changed' }, '', 30),
      { details: { field: 'expectedRevision', reason: 'missing' } },
    )
    await assert.rejects(
      skills.save(
        catalog,
        { id: 'one', expectedRevision: 2, name: 'guide', description: 'test', body: 'changed' },
        '',
        30,
      ),
      { details: { reason: 'revision_changed', entity: 'one' } },
    )
    catalog = await skills.save(
      catalog,
      { id: 'one', expectedRevision: 1, name: 'guide', description: 'test', body: 'changed' },
      '',
      30,
    )
    assert.throws(() => skills.snapshot(catalog, [{ id: 'one', revision: 1 }]))
    assert.equal(skills.snapshot(catalog, [{ id: 'one', revision: 2 }])[0]?.body, 'changed')
    catalog = await skills.duplicate(catalog, 'one', 'copy', 'two', 40)
    assert.throws(() => skills.reorder(catalog, ['one', 'one']))
    catalog = skills.reorder(catalog, ['two', 'one'])
    assert.deepEqual(
      skills.list(catalog).map((skill) => [skill.id, skill.revision]),
      [
        ['two', 1],
        ['one', 2],
      ],
    )
    catalog = await skills.syncBuiltins(
      catalog,
      [{ name: 'builtin', description: 'host', version: null, body: 'fixed' }],
      50,
    )
    assert.throws(() => skills.remove(catalog, 'builtin:builtin'))
    await assert.rejects(
      skills.save(
        catalog,
        { id: 'builtin:builtin', expectedRevision: 1, name: 'builtin', description: 'host', body: 'edit' },
        '',
        60,
      ),
      { details: { field: 'id', reason: 'builtin' } },
    )
    await assert.rejects(skills.duplicate(catalog, 'one', 'builtin', 'three', 60), {
      details: { reason: 'builtin_name' },
    })
    catalog = await skills.syncBuiltins(catalog, [], 60)
    assert.deepEqual(
      skills.list(catalog).map((skill) => skill.name),
      ['copy', 'guide'],
    )
  })
})
