// One quality gate for local checks and every native runner; no platform skips existing checks.
import { execFileSync, execSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

// Git for Windows ships a link.exe that shadows the MSVC linker; cargo goes through the wrapper.
const cargo = (args) => (process.platform === 'win32' ? `scripts\\cargo-msvc.bat ${args}` : `cargo ${args}`)

const steps = [
  'pnpm run check',
  // tauri::generate_context! in the examples needs each `frontendDist` to exist.
  'pnpm -r --filter "./examples/*/ui" run build',
  'pnpm run kit:check-deps',
  'pnpm run kit:dto-check',
  'pnpm run kit:i18n',
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
if (process.platform === 'darwin' || process.platform === 'linux') {
  execSync(cargo('build -p aether-providers --example native-secrets'), { stdio: 'inherit' })
  const fixture = mkdtempSync(join(tmpdir(), 'aether-native-secrets-'))
  const home = join(fixture, 'home')
  mkdirSync(home)
  const binary = resolve(process.env.CARGO_TARGET_DIR || 'target', 'debug/examples/native-secrets')
  const env = { ...process.env, HOME: home }
  const run = (mode, path, environment = env) =>
    execFileSync(binary, [mode, path], { stdio: 'inherit', env: environment })
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
    rmSync(fixture, { recursive: true, force: true })
  }
}
console.log('\nci: all steps passed')
