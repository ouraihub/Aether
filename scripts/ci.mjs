// Local CI defaults to the full gate; remote lightweight paths use explicit scopes.
import { execFileSync, execSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const scope =
  process.argv.length === 2 ? 'full' : process.argv[2] === '--scope' && process.argv.length === 4 ? process.argv[3] : ''
if (!['docs', 'automation', 'frontend', 'full'].includes(scope))
  throw new Error('Usage: ci.mjs [--scope docs|automation|frontend|full]')

// Git for Windows ships a link.exe that shadows the MSVC linker; cargo goes through desktop-kit.
const cargo = (args) =>
  process.platform === 'win32' ? `node_modules\\@ouraihub\\desktop-kit\\msvc\\cargo-msvc.bat ${args}` : `cargo ${args}`

const common = ['node --test scripts/ci-scope.test.mjs scripts/ci-gate.test.mjs', 'pnpm exec release-kit check']
const frontend = ['pnpm run check', 'pnpm -r --filter "./examples/*/ui" run build', 'pnpm run kit:i18n']
const steps =
  scope === 'docs' || scope === 'automation'
    ? common
    : scope === 'frontend'
      ? [...common, ...frontend]
      : [
          ...common,
          ...frontend,
          // tauri::generate_context! in the examples needs each frontendDist to exist.
          'pnpm run kit:check-deps',
          'pnpm run kit:dto-check',
          cargo('fmt --all -- --check'),
          cargo('clippy --workspace --all-targets --all-features -- -D warnings'),
          cargo('test --workspace --all-features'),
          ...['providers', 'chat', 'skills', 'mcp-client', 'mcp-server'].map((feature) =>
            cargo(`check -p tauri-plugin-aether --no-default-features --features ${feature}`),
          ),
        ]

for (const step of steps) {
  console.log(`\n=== ${step}`)
  execSync(step, { stdio: 'inherit' })
}
if (scope === 'full' && (process.platform === 'darwin' || process.platform === 'linux')) {
  execSync(cargo('build -p aether-providers --example native-secrets'), { stdio: 'inherit' })
  const fixture = mkdtempSync(join(tmpdir(), 'aether-native-secrets-'))
  const home = join(fixture, 'home')
  mkdirSync(home)
  const binary = resolve(process.env.CARGO_TARGET_DIR || 'target', 'debug/examples/native-secrets')
  const env = { ...process.env, ...(process.platform === 'linux' ? { HOME: home } : {}) }
  const run = (mode, path, environment = env) =>
    execFileSync(binary, [mode, path], { stdio: 'inherit', env: environment })
  let restoreKeychain
  if (process.platform === 'darwin') {
    const security = (args) => execFileSync('security', args, { encoding: 'utf8' }).trim()
    const original = security(['default-keychain', '-d', 'user']).replace(/^"|"$/g, '')
    const search = [...security(['list-keychains', '-d', 'user']).matchAll(/"([^"]+)"/g)].map((m) => m[1])
    const keychain = join(fixture, 'smoke.keychain-db')
    const password = crypto.randomUUID()
    security(['create-keychain', '-p', password, keychain])
    restoreKeychain = () => {
      const errors = []
      for (const args of [
        ['default-keychain', '-d', 'user', '-s', original],
        ['list-keychains', '-d', 'user', '-s', ...search],
        ['delete-keychain', keychain],
      ]) {
        try {
          security(args)
        } catch (error) {
          errors.push(error)
        }
      }
      if (errors.length) throw new AggregateError(errors, 'Cannot restore native test keychain')
    }
    try {
      security(['set-keychain-settings', '-lut', '21600', keychain])
      security(['unlock-keychain', '-p', password, keychain])
      security(['list-keychains', '-d', 'user', '-s', keychain])
      security(['default-keychain', '-d', 'user', '-s', keychain])
    } catch (error) {
      restoreKeychain()
      throw error
    }
  }
  try {
    // First access races in real separate processes; all resulting BLOBs must stay readable.
    run('race', join(fixture, 'race'))
    const file = join(fixture, 'saved.blob')
    run('seal', file)
    run('open', file)
    run('tamper', file)
    if (process.platform === 'linux') {
      run('unavailable', file, { ...env, DBUS_SESSION_BUS_ADDRESS: 'unix:path=/nonexistent/aether-bus' })
      run('open', file)
    }
    run('forget', file)
  } finally {
    restoreKeychain?.()
    rmSync(fixture, { recursive: true, force: true })
  }
}
console.log('\nci: all steps passed')
