// Local CI defaults to the full gate; remote lightweight paths use explicit scopes.
import { execFileSync, execSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, join, resolve } from 'node:path'

const scope =
  process.argv.length === 2 ? 'full' : process.argv[2] === '--scope' && process.argv.length === 4 ? process.argv[3] : ''
if (!['docs', 'automation', 'frontend', 'full'].includes(scope))
  throw new Error('Usage: ci.mjs [--scope docs|automation|frontend|full]')

// Git for Windows ships a link.exe that shadows the MSVC linker; cargo goes through desktop-kit.
const cargo = (args) =>
  process.platform === 'win32' ? `node_modules\\@ouraihub\\desktop-kit\\msvc\\cargo-msvc.bat ${args}` : `cargo ${args}`

const common = [
  'node --test scripts/ci-scope.test.mjs scripts/ci-gate.test.mjs scripts/scenario-check.test.mjs',
  'pnpm exec release-kit check',
]
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
          'pnpm --filter @ouraihub/aether-contracts check-generated',
          'pnpm --filter @ouraihub/aether-worker-example build',
          'pnpm run kit:check-deps',
          'pnpm run kit:nondet-check',
          'pnpm run kit:dto-check',
          'node scripts/contracts-check.mjs',
          cargo('check -p aether-support --no-default-features'),
          cargo('check -p aether-chat --no-default-features'),
          cargo('check -p aether-images --no-default-features'),
          cargo('check -p aether-runtime --no-default-features --features conversations'),
          cargo('fmt --all -- --check'),
          cargo('clippy --workspace --all-targets --all-features -- -D warnings'),
          // nextest gives every test its own process and a hang timeout; it does not run doctests.
          cargo('nextest run --workspace --all-features'),
          cargo('test --workspace --all-features --doc'),
          ...['providers', 'chat', 'conversations', 'skills', 'search', 'mcp-client', 'mcp-server'].map((feature) =>
            cargo(`check -p tauri-plugin-aether --no-default-features --features ${feature}`),
          ),
        ]

// Pinned release tools are fetched into a throwaway directory and run by path, never installed globally.
const directories = []
function release(name, url) {
  const directory = mkdtempSync(join(tmpdir(), `aether-${name}-`))
  directories.push(directory)
  const archive = join(directory, url.slice(url.lastIndexOf('/') + 1))
  execFileSync('curl', ['--fail', '--location', '--max-time', '120', '--output', archive, url], { stdio: 'inherit' })
  execFileSync('tar', ['-xf', archive, '-C', directory], { stdio: 'inherit' })
  return directory
}

try {
  if (scope !== 'docs') {
    const platform = { win32: 'windows', darwin: 'darwin', linux: 'linux' }[process.platform]
    const arch = { x64: 'amd64', arm64: 'arm64' }[process.arch]
    if (!platform || !arch) throw new Error('Unsupported actionlint platform')
    const archive = `actionlint_1.7.12_${platform}_${arch}.${platform === 'windows' ? 'zip' : 'tar.gz'}`
    const directory = release('actionlint', `https://github.com/rhysd/actionlint/releases/download/v1.7.12/${archive}`)
    execFileSync(
      join(directory, platform === 'windows' ? 'actionlint.exe' : 'actionlint'),
      ['-shellcheck=', '-pyflakes='],
      {
        stdio: 'inherit',
      },
    )
  }
  if (scope === 'full') {
    const target = {
      'win32-x64': 'x86_64-pc-windows-msvc.zip',
      'win32-arm64': 'aarch64-pc-windows-msvc.zip',
      'darwin-x64': 'universal-apple-darwin.tar.gz',
      'darwin-arm64': 'universal-apple-darwin.tar.gz',
      'linux-x64': 'x86_64-unknown-linux-gnu.tar.gz',
      'linux-arm64': 'aarch64-unknown-linux-gnu.tar.gz',
    }[`${process.platform}-${process.arch}`]
    if (!target) throw new Error('Unsupported cargo-nextest platform')
    const version = 'cargo-nextest-0.9.149'
    const nextest = release(
      'nextest',
      `https://github.com/nextest-rs/nextest/releases/download/${version}/${version}-${target}`,
    )
    // cargo resolves `cargo nextest` through PATH; the MSVC wrapper only prepends to PATH.
    process.env.PATH = `${nextest}${delimiter}${process.env.PATH}`
  }

  for (const step of steps) {
    console.log(`\n=== ${step}`)
    execSync(step, { stdio: 'inherit' })
  }
} finally {
  for (const directory of directories) rmSync(directory, { recursive: true, force: true })
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
