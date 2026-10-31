// Compile public schemas ahead of time: runtime consumers never compile schemas or evaluate code.
import { readFileSync, readdirSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import Ajv from 'ajv'
import standaloneCode from 'ajv/dist/standalone/index.js'
import ts from 'typescript'

const directory = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = resolve(directory, '../..')
const generated = join(directory, 'src/generated')
const manifest = JSON.parse(readFileSync(join(root, 'contracts/manifest.json'), 'utf8'))
const declarations = new Map()
const names = []
for (const file of [...readdirSync(generated).filter((file) => file.endsWith('.ts')).map((file) => join(generated, file)), join(directory, 'src/search.ts')]) {
  const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true)
  for (const node of source.statements) {
    if (ts.isTypeAliasDeclaration(node) || ts.isInterfaceDeclaration(node)) {
      declarations.set(node.name.text, node)
      names.push(node.name.text)
    }
  }
}
function schema(node) {
  switch (node.kind) {
    case ts.SyntaxKind.StringKeyword: return { type: 'string' }
    case ts.SyntaxKind.NumberKeyword: return { type: 'number' }
    case ts.SyntaxKind.BooleanKeyword: return { type: 'boolean' }
    case ts.SyntaxKind.UnknownKeyword: return {}
    case ts.SyntaxKind.NullKeyword: return { type: 'null' }
  }
  if (ts.isLiteralTypeNode(node)) {
    const literal = node.literal
    if (literal.kind === ts.SyntaxKind.NullKeyword) return { type: 'null' }
    if (literal.kind === ts.SyntaxKind.TrueKeyword) return { const: true }
    if (literal.kind === ts.SyntaxKind.FalseKeyword) return { const: false }
    if (ts.isStringLiteral(literal)) return { const: literal.text }
    if (ts.isNumericLiteral(literal)) return { const: Number(literal.text) }
  }
  if (ts.isUnionTypeNode(node)) return { anyOf: node.types.map(schema) }
  if (ts.isArrayTypeNode(node)) return { type: 'array', items: schema(node.elementType) }
  if (ts.isTypeReferenceNode(node)) {
    const name = node.typeName.getText()
    if (name === 'Array' || name === 'ReadonlyArray') return { type: 'array', items: schema(node.typeArguments[0]) }
    if (name === 'Record') return { type: 'object', additionalProperties: schema(node.typeArguments[1]) }
    if (!declarations.has(name)) throw new Error(`Unknown DTO type ${name}`)
    return { $ref: `aether:bindings#/definitions/${name}` }
  }
  if (ts.isMappedTypeNode(node)) return { type: 'object', additionalProperties: schema(node.type) }
  if (ts.isTypeLiteralNode(node) || ts.isInterfaceDeclaration(node)) {
    const properties = {}
    const required = []
    for (const member of node.members) {
      if (!ts.isPropertySignature(member) || !member.type || !member.name) throw new Error(`Unsupported DTO member ${member.getText()}`)
      const name = ts.isIdentifier(member.name) || ts.isStringLiteral(member.name) ? member.name.text : undefined
      if (!name) throw new Error('Unsupported DTO property name')
      properties[name] = schema(member.type)
      if (!member.questionToken) required.push(name)
    }
    return { type: 'object', properties, required, additionalProperties: false }
  }
  if (ts.isParenthesizedTypeNode(node)) return schema(node.type)
  throw new Error(`Unsupported DTO syntax: ${node.getText()}`)
}
const definitions = Object.fromEntries([...declarations].map(([name, node]) => [name, schema(ts.isTypeAliasDeclaration(node) ? node.type : node)]))
const ajv = new Ajv({ strict: true, allErrors: true, code: { source: true, esm: true } })
ajv.addSchema({ $id: 'aether:bindings', definitions })
const normative = new Map()
for (const file of manifest.schemas) {
  const value = JSON.parse(readFileSync(join(root, 'contracts', file), 'utf8'))
  normative.set(file, value)
  ajv.addSchema(value)
}
const exports = {}
for (const name of names.sort()) {
  const contract = manifest.contracts[name]
  const ref = contract ? `${normative.get(contract.schema).$id}#/definitions/${contract.definition}` : `aether:bindings#/definitions/${name}`
  const id = `aether:validator:${name}`
  ajv.addSchema({ $id: id, $ref: ref })
  exports[`validate${name}`] = id
}
let compiled = standaloneCode(ajv, exports)
const imports = new Map()
compiled = compiled.replace(/require\("(ajv\/dist\/runtime\/[^" ]+)"\)\.default/g, (_, path) => {
  if (!imports.has(path)) imports.set(path, `runtime${imports.size}`)
  return imports.get(path)
})
const require = createRequire(import.meta.url)
compiled = [...imports].map(([path, name]) => `const ${name} = ${require(path).default.toString()};`).join('\n') + '\n' + compiled + '\n'
const typeImport = (name) => {
  const path = name.startsWith('Search') && name !== 'SearchHit' ? './search' : `./generated/${name}`
  const line = `import type { ${name} } from '${path}'`
  return line.length <= 80 ? line : `import type {\n  ${name},\n} from '${path}'`
}
const validatorExport = (name) => {
  const line = `export const validate${name}: ContractValidator<${name}> = validators.${name}`
  return line.length <= 80
    ? line
    : `export const validate${name}: ContractValidator<${name}> =\n  validators.${name}`
}
const declarationExport = (name) => {
  const line = `export const validate${name}: ValidateFunction<${name}>`
  return line.length <= 80
    ? line
    : `export const validate${name}: ValidateFunction<\n  ${name}\n>`
}
const validationImports = names.map(typeImport)
const validationExports = names.map(validatorExport)
const index = `${readdirSync(generated).filter((file) => file.endsWith('.ts')).sort().map((file) => `export type * from './generated/${file.slice(0, -3)}'`).join('\n')}\nexport type * from './search'\nexport * from './validation'\n`
const validation = `// Generated by scripts/generate.mjs; normative run/error/conversation schemas take precedence.\nimport type { ErrorObject } from 'ajv'\n${validationImports.join('\n')}\nimport * as compiled from './validators.js'\n\nexport interface ContractMap {\n${names.map((name) => `  ${name}: ${name}`).join('\n')}\n}\nexport type ContractName = keyof ContractMap\nexport type ContractValidator<T> = {\n  (value: unknown): value is T\n  errors?: ErrorObject[] | null\n}\nconst validators: { [N in ContractName]: ContractValidator<ContractMap[N]> } = {\n${names.map((name) => `  ${name}: compiled.validate${name}`).join(',\n')}\n}\nexport function isContract<N extends ContractName>(\n  name: N,\n  value: unknown,\n): value is ContractMap[N] {\n  return validators[name](value)\n}\nexport function contractErrors(name: ContractName): readonly ErrorObject[] {\n  return validators[name].errors ?? []\n}\n${validationExports.join('\n')}\n`
const declarationsText = `import type { ValidateFunction } from 'ajv'\n${validationImports.join('\n')}\n${declarationExports.join('\n')}\n`
for (const [file, text] of [['src/validators.js', compiled], ['src/validators.d.ts', declarationsText], ['src/index.ts', index], ['src/validation.ts', validation]]) {
  const path = join(directory, file)
  if (process.argv.includes('--check')) {
    if (readFileSync(path, 'utf8') !== text) throw new Error(`${file} is stale; run contracts generate`)
  } else {
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, text)
  }
}
