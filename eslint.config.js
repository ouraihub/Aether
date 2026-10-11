// Rules map to docs/04-工程规范.md §10.
import js from '@eslint/js'
import globals from 'globals'
import importX, { createNodeResolver } from 'eslint-plugin-import-x'
import svelte from 'eslint-plugin-svelte'
import tseslint from 'typescript-eslint'

const COLOR_SCALES = String.raw`(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)`
const COLOR_PREFIXES = String.raw`(?:bg|text|border|ring|outline|divide|from|via|to|shadow|decoration|accent|caret|fill|stroke|placeholder)`
const SCALE_CLASS = new RegExp(String.raw`\b${COLOR_PREFIXES}-${COLOR_SCALES}-\d{2,3}\b`)
const ARBITRARY_CLASS = new RegExp(String.raw`\b${COLOR_PREFIXES}-\[(?:#|rgba?\(|hsla?\(|okl(?:ch|ab)\()`, 'i')

// shadcn-svelte rule: semantic color classes only (bg-primary, text-muted-foreground), so hosts can retheme (docs/16 §3).
const noHardcodedColor = {
  meta: { type: 'problem', schema: [], messages: { color: 'Use a semantic color class instead of {{value}}.' } },
  create(context) {
    const check = (node, value) => {
      const match = SCALE_CLASS.exec(value) ?? ARBITRARY_CLASS.exec(value)
      if (match) context.report({ node, messageId: 'color', data: { value: match[0] } })
    }
    return {
      Literal: (node) => typeof node.value === 'string' && check(node, node.value),
      TemplateElement: (node) => check(node, node.value.raw),
      SvelteLiteral: (node) => typeof node.value === 'string' && check(node, node.value),
    }
  },
}

const UI_SOURCE = ['packages/ui/src/**/*.{ts,svelte}']

export default [
  { ignores: ['**/node_modules/**', '**/target/**', '**/dist/**', '**/*.d.ts', 'packages/ui/src/ports/generated/**'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...svelte.configs.recommended,
  {
    files: UI_SOURCE,
    languageOptions: {
      globals: globals.browser,
      parserOptions: {
        parser: tseslint.parser,
        extraFileExtensions: ['.svelte'],
        project: ['./packages/ui/tsconfig.json'],
      },
    },
    plugins: { 'import-x': importX, kit: { rules: { 'no-hardcoded-color': noHardcodedColor } } },
    settings: {
      'import-x/resolver-next': [createNodeResolver({ extensions: ['.ts', '.svelte.ts', '.svelte', '.js'] })],
      // Without this import-x only follows .js files, and no-cycle silently misses TS cycles.
      'import-x/extensions': ['.ts', '.svelte', '.js'],
      // Per-extension parsers, or no-cycle fails to parse (and skips) imported .svelte files and node_modules JS.
      'import-x/parsers': { '@typescript-eslint/parser': ['.ts', '.js'], 'svelte-eslint-parser': ['.svelte'] },
    },
    rules: {
      'no-console': 'error',
      'no-empty': ['error', { allowEmptyCatch: false }],
      '@typescript-eslint/no-explicit-any': 'error',
      '@typescript-eslint/no-floating-promises': 'error',
      '@typescript-eslint/no-misused-promises': 'error',
      '@typescript-eslint/no-non-null-assertion': 'error',
      '@typescript-eslint/consistent-type-imports': 'error',
      'import-x/no-cycle': 'error',
      'svelte/no-at-html-tags': 'error',
      'kit/no-hardcoded-color': 'error',
    },
  },
  // Only src/tauri/ may touch Tauri APIs, so components and ports stay testable without Tauri (docs/16 §1).
  {
    files: UI_SOURCE,
    ignores: ['packages/ui/src/tauri/**'],
    rules: {
      'no-restricted-imports': [
        'error',
        { patterns: [{ group: ['@tauri-apps/*'], message: 'Only packages/ui/src/tauri may import Tauri APIs.' }] },
      ],
    },
  },
]
