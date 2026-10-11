// Enforces the crate dependency rules in docs/00-概要设计.md §3.
// Prints one line per violation and exits 1; exits 0 when the graph is clean.
import { execFileSync } from 'node:child_process'

const SUPPORT_ONLY = ['aether-support']

// Which kit crates each workspace crate may depend on directly. `null` = any kit crate.
const ALLOWED_KIT_DEPS = new Map([
  ['aether-support', []],
  ['aether-observability', SUPPORT_ONLY],
  ['aether-providers', SUPPORT_ONLY],
  ['aether-chat', SUPPORT_ONLY],
  ['aether-skills', SUPPORT_ONLY],
  ['aether-mcp-client', SUPPORT_ONLY],
  ['aether-mcp-server', SUPPORT_ONLY],
  ['tauri-plugin-aether', null],
])

// Must not reach the `tauri` crate through normal dependencies, so they stay usable from a CLI.
const NO_TAURI = [
  'aether-support',
  'aether-observability',
  'aether-providers',
  'aether-chat',
  'aether-skills',
  'aether-mcp-client',
  'aether-mcp-server',
  'example-cli',
]

// aether-support is private: hosts (the examples) reach it only through the public crates.
const PRIVATE = ['aether-support']

const isKitCrate = (name) => name.startsWith('aether-') || name === 'tauri-plugin-aether'

const metadata = JSON.parse(
  execFileSync('cargo', ['metadata', '--format-version', '1', '--all-features'], {
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
  }),
)

const byId = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]))
const members = metadata.workspace_members.map((id) => byId.get(id))
const memberNames = new Set(members.map((pkg) => pkg.name))
const violations = []

for (const pkg of members) {
  const kitDeps = pkg.dependencies.filter((dep) => memberNames.has(dep.name) && isKitCrate(dep.name))

  if (isKitCrate(pkg.name)) {
    if (!ALLOWED_KIT_DEPS.has(pkg.name)) {
      violations.push(`${pkg.name}: not listed in scripts/check-deps.mjs ALLOWED_KIT_DEPS`)
      continue
    }
    const allowed = ALLOWED_KIT_DEPS.get(pkg.name)
    for (const dep of kitDeps) {
      if (allowed !== null && !allowed.includes(dep.name)) {
        violations.push(
          `${pkg.name} -> ${dep.name}: not allowed (may depend on: ${allowed.join(', ') || 'no kit crate'})`,
        )
      }
    }
  } else {
    for (const dep of kitDeps) {
      if (PRIVATE.includes(dep.name))
        violations.push(`${pkg.name} -> ${dep.name}: private crate, use the public kit crates`)
    }
  }
}

// Normal-dependency closure from the resolve graph; dev and build dependencies do not ship.
const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]))
const normalDeps = (id) =>
  (nodes.get(id)?.deps ?? []).filter((dep) => dep.dep_kinds.some((kind) => kind.kind === null)).map((dep) => dep.pkg)

for (const pkg of members.filter((m) => NO_TAURI.includes(m.name))) {
  const parent = new Map([[pkg.id, null]])
  const queue = [pkg.id]
  while (queue.length > 0) {
    const id = queue.shift()
    if (byId.get(id).name === 'tauri') {
      const chain = []
      for (let at = id; at !== null; at = parent.get(at)) chain.unshift(byId.get(at).name)
      violations.push(`${pkg.name}: must not depend on tauri (${chain.join(' -> ')})`)
      break
    }
    for (const next of normalDeps(id)) {
      if (!parent.has(next)) {
        parent.set(next, id)
        queue.push(next)
      }
    }
  }
}

if (violations.length > 0) {
  for (const line of violations) process.stderr.write(`check-deps: ${line}\n`)
  process.exit(1)
}
process.stdout.write(`check-deps: ${members.length} workspace crates OK\n`)
