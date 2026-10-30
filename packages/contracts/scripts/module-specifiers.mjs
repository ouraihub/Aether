import { extname } from 'node:path'
import ts from 'typescript'

export function relativeJsImports(text) {
  const source = ts.createSourceFile('contracts.ts', text, ts.ScriptTarget.Latest, true)
  for (const node of [...source.statements].reverse()) {
    if (!ts.isImportDeclaration(node) && !ts.isExportDeclaration(node)) continue
    const specifier = node.moduleSpecifier
    if (!specifier || !ts.isStringLiteral(specifier)) continue
    if (!specifier.text.startsWith('./') && !specifier.text.startsWith('../')) continue
    if (extname(specifier.text)) continue
    const end = specifier.getEnd() - 1
    text = `${text.slice(0, end)}.js${text.slice(end)}`
  }
  return text
}
