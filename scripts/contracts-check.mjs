// docs/22 gate: AJV fixtures, TypeScript binding assignments and actual Rust serialization.
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import Ajv from 'ajv'
import ts from 'typescript'
import * as standalone from '../packages/contracts/src/validators.js'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const directory = join(root, 'contracts')
const json = (path) => JSON.parse(readFileSync(path, 'utf8'))

function main() {
  const args = process.argv.slice(2)
  if (args.length !== 0 && (args.length !== 2 || args[0] !== '--rust-output')) {
    throw new Error('Usage: node scripts/contracts-check.mjs [--rust-output <file|->]')
  }
  execFileSync(process.execPath, ['packages/contracts/scripts/generate.mjs', '--check'], {
    cwd: root,
    stdio: 'inherit',
  })
  const manifest = json(join(directory, 'manifest.json'))
  const fixtures = json(join(directory, 'cases.json'))
  if (manifest.contractVersion !== 1 || fixtures.contractVersion !== 1) {
    throw new Error('Unsupported contractVersion')
  }
  const ajv = new Ajv({ strict: true, allErrors: true })
  const schemas = new Map()
  for (const file of manifest.schemas) {
    const schema = json(join(directory, file))
    if (schema.$schema !== manifest.schemaDraft) throw new Error(`${file}: wrong schema draft`)
    schemas.set(file, schema)
    ajv.addSchema(schema)
  }
  const validators = new Map()
  for (const [name, contract] of Object.entries(manifest.contracts)) {
    const schema = schemas.get(contract.schema)
    validators.set(name, ajv.compile({ $ref: `${schema.$id}#/definitions/${contract.definition}` }))
  }
  const check = (contract, value, expected, label) => {
    const validate = validators.get(contract)
    if (!validate) throw new Error(`${label}: unknown contract ${contract}`)
    let actual
    try {
      actual = Boolean(validate(value))
      const compiled = standalone[`validate${contract}`]
      if (typeof compiled !== 'function' || Boolean(compiled(value)) !== actual)
        throw new Error(`${label}: standalone ${contract} differs from the schema validator`)
    } catch (error) {
      throw new Error(`${label}: ${contract} validator threw ${error.name}: ${error.message}`, { cause: error })
    }
    if (actual !== expected) {
      throw new Error(
        `${label}: expected ${expected ? 'valid' : 'invalid'} ${contract}; ${ajv.errorsText(validate.errors)}`,
      )
    }
  }
  const cases = new Map()
  for (const fixture of fixtures.cases) {
    if (cases.has(fixture.id)) throw new Error(`Duplicate fixture ${fixture.id}`)
    if (typeof fixture.valid !== 'boolean' || (fixture.tsReject && fixture.valid)) {
      throw new Error(`${fixture.id}: invalid fixture expectation`)
    }
    cases.set(fixture.id, fixture)
    check(fixture.contract, fixture.value, fixture.valid, fixture.id)
  }
  for (const name of validators.keys()) {
    const values = fixtures.cases.filter((fixture) => fixture.contract === name)
    if (!values.some((fixture) => fixture.valid) || !values.some((fixture) => !fixture.valid)) {
      throw new Error(`${name}: requires positive and negative shared cases`)
    }
  }
  let boundaries = 0
  for (const boundary of fixtures.boundaries) {
    const base = cases.get(boundary.base)
    if (!base || base.contract !== boundary.contract || !base.valid) {
      throw new Error(`${boundary.id}: invalid boundary base`)
    }
    for (const length of [boundary.limit, boundary.limit + 1]) {
      const value = structuredClone(base.value)
      let target = value
      for (const key of boundary.path.slice(0, -1)) target = target[key]
      target[boundary.path.at(-1)] = boundary.unit.repeat(length)
      check(boundary.contract, value, length === boundary.limit, `${boundary.id}/${length}`)
      boundaries++
    }
  }
  // A 4 MiB decoded input needs padding at the maximum encoded length. One extra byte
  // has the same encoded length, so maxLength alone does not enforce the byte budget.
  for (const bytes of [4 * 1024 * 1024, 4 * 1024 * 1024 + 1]) {
    check(
      'ImagePart',
      { mime: 'image/png', data: Buffer.alloc(bytes).toString('base64') },
      bytes === 4 * 1024 * 1024,
      `image-decoded-bytes/${bytes}`,
    )
    boundaries++
  }
  const maximumImage = Buffer.alloc(4 * 1024 * 1024).toString('base64')
  for (const [suffix, data] of [
    ['newline', `${maximumImage}\n`],
    ['noncanonical-padding', `${maximumImage.slice(0, -3)}B==`],
    ['invalid-alphabet', `${maximumImage.slice(0, -4)}!A==`],
  ]) {
    check('ImagePart', { mime: 'image/png', data }, false, `image-large-${suffix}`)
    boundaries++
  }

  // TypeScript itself checks fresh object literal assignments (including excess fields).
  // Numeric ranges, string lengths, provenance and dependent states remain schema/behavior rules.
  const imports = [...validators.keys()].map(
    (name) => `import type { ${name} } from ${JSON.stringify(`./packages/contracts/src/generated/${name}.js`)};`,
  )
  let text = `${imports.join('\n')}\n`
  const bindingRanges = []
  for (const fixture of fixtures.cases) {
    if (!fixture.valid && !fixture.tsReject) continue
    const start = text.length
    text += `const fixture${bindingRanges.length}: ${fixture.contract} = ${JSON.stringify(fixture.value)};\n`
    bindingRanges.push({ fixture, start, end: text.length, diagnostics: [] })
  }
  const bindingCases = bindingRanges.length
  const virtual = join(root, '__aether_contract_fixtures__.ts')
  const options = {
    noEmit: true,
    strict: true,
    exactOptionalPropertyTypes: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    types: [],
    skipLibCheck: false,
  }
  const host = ts.createCompilerHost(options)
  const canonicalPath = (path) => {
    const normalized = resolve(path).replaceAll('\\', '/')
    return host.useCaseSensitiveFileNames() ? normalized : normalized.toLowerCase()
  }
  const virtualPath = canonicalPath(virtual)
  const isVirtual = (path) => canonicalPath(path) === virtualPath
  const originalGetSourceFile = host.getSourceFile.bind(host)
  const originalFileExists = host.fileExists.bind(host)
  const originalReadFile = host.readFile.bind(host)
  host.fileExists = (path) => isVirtual(path) || originalFileExists(path)
  host.readFile = (path) => (isVirtual(path) ? text : originalReadFile(path))
  host.getSourceFile = (path, languageVersion, onError, shouldCreateNewSourceFile) =>
    isVirtual(path)
      ? ts.createSourceFile(path, text, languageVersion, true)
      : originalGetSourceFile(path, languageVersion, onError, shouldCreateNewSourceFile)
  const program = ts.createProgram([virtual], options, host)
  const diagnostics = ts.getPreEmitDiagnostics(program)
  const unexpected = []
  for (const diagnostic of diagnostics) {
    const range =
      diagnostic.file && isVirtual(diagnostic.file.fileName) && diagnostic.start !== undefined
        ? bindingRanges.find((entry) => diagnostic.start >= entry.start && diagnostic.start < entry.end)
        : undefined
    if (range && range.fixture.tsReject) range.diagnostics.push(diagnostic)
    else unexpected.push(diagnostic)
  }
  const wronglyAccepted = bindingRanges.filter((entry) => entry.fixture.tsReject && entry.diagnostics.length === 0)
  if (unexpected.length || wronglyAccepted.length) {
    const formatted = ts.formatDiagnosticsWithColorAndContext(unexpected, {
      getCanonicalFileName: canonicalPath,
      getCurrentDirectory: () => root,
      getNewLine: () => '\n',
    })
    throw new Error(
      `Generated bindings differ from shared fixtures; run pnpm kit:dto first.\n${formatted}${wronglyAccepted.map((entry) => `${entry.fixture.id}: invalid fixture accepted by ${entry.fixture.contract}`).join('\n')}`,
    )
  }

  let rustCount = 0
  {
    let serialized
    if (args.length) {
      serialized = readFileSync(args[1] === '-' ? 0 : resolve(args[1]), 'utf8')
    } else {
      const cargo = ['build', '-p', 'aether-runtime', '--features', 'runs', '--example', 'contract-output', '--quiet']
      const [command, cargoArgs] =
        process.platform === 'win32'
          ? ['cmd.exe', ['/d', '/c', 'node_modules\\@ouraihub\\desktop-kit\\msvc\\cargo-msvc.bat', ...cargo]]
          : ['cargo', cargo]
      execFileSync(command, cargoArgs, { cwd: root, stdio: 'inherit' })
      const executable = join(
        resolve(root, process.env.CARGO_TARGET_DIR || 'target'),
        'debug',
        'examples',
        process.platform === 'win32' ? 'contract-output.exe' : 'contract-output',
      )
      serialized = execFileSync(executable, [], {
        cwd: root,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'inherit'],
        maxBuffer: 16 * 1024 * 1024,
      })
    }
    const output = JSON.parse(serialized)
    if (!Array.isArray(output) || output.length === 0) throw new Error('Rust output must be a nonempty JSON array')
    const contracts = new Set()
    const kinds = new Set()
    const statuses = new Set()
    for (const [index, item] of output.entries()) {
      if (
        !item ||
        typeof item !== 'object' ||
        Array.isArray(item) ||
        Object.keys(item).some((key) => !['contract', 'value'].includes(key)) ||
        !Object.hasOwn(item, 'value')
      )
        throw new Error(`Rust output ${index}: expected {contract,value}`)
      check(item.contract, item.value, true, `Rust output ${index}`)
      contracts.add(item.contract)
      if (item.contract === 'RunEvent') kinds.add(item.value.kind)
      if (item.contract === 'RunSnapshot') statuses.add(item.value.status)
      rustCount++
    }
    for (const [label, expected, actual] of [
      ['contract', manifest.rustOutput.requiredContracts, contracts],
      ['event kind', manifest.rustOutput.requiredEventKinds, kinds],
      ['snapshot status', manifest.rustOutput.requiredSnapshotStatuses, statuses],
    ]) {
      for (const value of expected) if (!actual.has(value)) throw new Error(`Rust output missing ${label}: ${value}`)
    }
  }
  process.stdout.write(
    `contracts-check: ${validators.size} Draft07 contracts, ${cases.size} shared cases, ${boundaries} boundaries, ${bindingCases} TypeScript assignments passed\n`,
  )
  process.stdout.write(
    `contracts-check: ${rustCount} actual Rust serialization values passed; behavior scenarios are specified separately, not executed by this data gate\n`,
  )
}

try {
  main()
} catch (error) {
  process.stderr.write(`contracts-check: ${error.message}\n`)
  // Only stack frame lines: do not dump attached values, child-process stdout or causes.
  for (let current = error; current instanceof Error; current = current.cause) {
    const frames = current.stack
      ?.split('\n')
      .filter((line) => /^\s+at /.test(line))
      .join('\n')
    if (frames) process.stderr.write(`${frames}\n`)
  }
  process.exitCode = 1
}
