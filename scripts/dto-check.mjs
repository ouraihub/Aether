// Regenerates the ts-rs DTOs (docs/00 §7.5) into a temp dir and compares them with the committed
// packages/ui/src/ports/generated. Exits 1 on any missing, extra or changed file.
// `--write` replaces the committed dir with the fresh output instead.
import { execFileSync } from 'node:child_process'
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const committed = join(root, 'packages/ui/src/ports/generated')
const fresh = mkdtempSync(join(tmpdir(), 'kit-dto-'))

// An explicit TS_RS_EXPORT_DIR wins over .cargo/config.toml [env] (no `force`).
const test = ['test', '--workspace', '--all-features', 'export_bindings']
const [command, args] =
  process.platform === 'win32'
    ? ['cmd.exe', ['/d', '/c', 'node_modules\\@ouraihub\\msvc-kit\\cargo-msvc.bat', ...test]]
    : ['cargo', test]

try {
  execFileSync(command, args, {
    cwd: root,
    env: { ...process.env, TS_RS_EXPORT_DIR: fresh },
    stdio: ['ignore', 'ignore', 'inherit'],
  })
  if (process.argv.includes('--write')) {
    rmSync(committed, { recursive: true, force: true })
    cpSync(fresh, committed, { recursive: true })
    process.stdout.write(`dto-check: wrote ${readdirSync(fresh).length} files\n`)
  } else {
    const list = (dir) => new Set(readdirSync(dir, { recursive: true }).filter((f) => f.endsWith('.ts')))
    const want = list(fresh)
    const have = list(committed)
    const problems = [
      ...[...want].filter((f) => !have.has(f)).map((f) => `missing ${f}`),
      ...[...have].filter((f) => !want.has(f)).map((f) => `stale ${f}`),
      ...[...want]
        .filter((f) => have.has(f) && readFileSync(join(fresh, f), 'utf8') !== readFileSync(join(committed, f), 'utf8'))
        .map((f) => `changed ${f}`),
    ]
    if (problems.length > 0) {
      for (const p of problems) process.stderr.write(`dto-check: ${p}\n`)
      process.stderr.write('dto-check: run `pnpm kit:dto` and commit the result\n')
      process.exitCode = 1
    } else {
      process.stdout.write(`dto-check: ${want.size} generated files match\n`)
    }
  }
} finally {
  rmSync(fresh, { recursive: true, force: true })
}
