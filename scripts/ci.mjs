// The one quality gate: GitHub CI (windows job) and local pre-push both run this file,
// so a step can never exist in one place and be missing in the other.
import { execSync } from 'node:child_process'

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
console.log('\nci: all steps passed')
