import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const script = fileURLToPath(new URL('./file-size.mjs', import.meta.url))

test('file size gate covers existing Rust, UI, plugin and untracked production files', () => {
  const root = mkdtempSync(join(tmpdir(), 'aether-file-size-'))
  const put = (file, lines) => {
    const path = join(root, file)
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, '// implementation\n'.repeat(lines))
  }
  const run = () => spawnSync(process.execPath, [script], { cwd: root, encoding: 'utf8' })
  try {
    execFileSync('git', ['init', '--quiet'], { cwd: root })
    const tracked = [
      'crates/demo/src/store.rs',
      'packages/ui/src/Panel.svelte',
      'plugin/demo/src/lib.rs',
    ]
    for (const file of tracked) put(file, 500)
    put('crates/demo/src/deleted.rs', 501)
    execFileSync('git', ['add', '.'], { cwd: root })
    rmSync(join(root, 'crates/demo/src/deleted.rs'))
    assert.equal(run().status, 0, '500 physical lines and deleted files are valid')

    for (const file of [...tracked, 'packages/worker/src/new.ts']) put(file, 501)
    const failed = run()
    assert.equal(failed.status, 1)
    for (const file of [...tracked, 'packages/worker/src/new.ts'])
      assert.ok(failed.stderr.includes(`${file}: 501 > 500`), failed.stderr)

    for (const file of [...tracked, 'packages/worker/src/new.ts']) put(file, 500)
    for (const file of [
      'crates/demo/src/tests.rs',
      'crates/demo/src/sources_tests.rs',
      'crates/demo/src/store/tests.rs',
      'packages/ui/src/Panel.test.ts',
      'packages/core/tests/traces.ts',
      'packages/contracts/src/generated/Dto.ts',
      'packages/contracts/src/validators.js',
      'packages/ui/src/i18n/en.ts',
      'crates/aether-skills/src/creator/scripts/resource.py',
    ])
      put(file, 700)
    assert.equal(run().status, 0, 'named test, generated and resource exceptions remain valid')
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
