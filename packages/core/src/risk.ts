export interface Risk {
  reason:
    | 'delete'
    | 'disk'
    | 'git_destructive'
    | 'permissions'
    | 'publish'
    | 'global_install'
    | 'elevation'
    | 'download_exec'
    | 'home_path'
    | 'process_spawn'
    | 'network'
    | 'file_delete'
}
export type ScriptLanguage =
  | 'shell'
  | 'powershell'
  | 'python'
  | 'javascript'
/** Advisory only: aliases and encoding can evade detection. */
export function shellRisk(command: string): Risk | null {
  const rules: readonly [Risk['reason'], RegExp][] = [
    [
      'download_exec',
      new RegExp(
        '(?:curl|wget|invoke-webrequest|iwr)\\b[^\\n]*(?:\\||;|&&)' +
          '[^\\n]*(?:sh|bash|pwsh|powershell|iex|invoke-expression)\\b',
        'i',
      ),
    ],
    [
      'elevation',
      /(?:^|[\s;&|])(sudo|doas|runas)(?:\s|$)|start-process\b.*-verb\s+runas/i,
    ],
    ['delete', /(?:^|[\s;&|])(rm|rmdir|del|erase|remove-item)(?:\s|$)/i],
    [
      'disk',
      new RegExp(
        '(?:^|[\\s;&|])(mkfs(?:\\.\\w+)?|fdisk|diskpart|format)' +
          '(?:\\s|$)|\\bdd\\b.*\\bof=',
        'i',
      ),
    ],
    [
      'git_destructive',
      new RegExp(
        '\\bgit\\s+(?:reset\\s+--hard|clean\\b[^\\n]*-[^\\s]*f|' +
          'push\\b[^\\n]*(?:--force|-f\\b)|checkout\\s+--)',
        'i',
      ),
    ],
    ['permissions', /\b(?:chmod|chown|icacls|set-acl)\b/i],
    [
      'publish',
      /\b(?:npm|pnpm|yarn|cargo|dotnet)\s+(?:publish|nuget\s+push)\b/i,
    ],
    [
      'global_install',
      new RegExp(
        '\\b(?:npm|pnpm|yarn)\\b[^\\n]*(?:--global|\\s-g\\b)|' +
          '\\bpip\\b[^\\n]*\\binstall\\b(?![^\\n]*--target)',
        'i',
      ),
    ],
    [
      'home_path',
      new RegExp(
        '(?:^|\\s)~(?:[\\\\/]|$)|\\$(?:HOME|USERPROFILE)\\b|' +
          '%(?:USERPROFILE|HOMEPATH)%',
        'i',
      ),
    ],
    [
      'process_spawn',
      new RegExp(
        '\\$\\(|`[^`]+`|\\b(?:eval|exec|cmd\\s+\\/c|' +
          'powershell\\s+-|bash\\s+-c|sh\\s+-c)\\b',
        'i',
      ),
    ],
  ]
  for (const [reason, pattern] of rules)
    if (pattern.test(command)) return { reason }
  return null
}
export function scriptRisk(
  language: ScriptLanguage,
  script: string,
): Risk | null {
  if (language === 'shell' || language === 'powershell')
    return shellRisk(script)
  if (
    new RegExp(
      '\\b(?:subprocess|os\\.(?:system|popen)|child_process|Bun\\.spawn|' +
        'Deno\\.Command)\\b',
    ).test(script)
  )
    return { reason: 'process_spawn' }
  if (
    new RegExp(
      '\\b(?:requests\\.|urllib\\.|httpx\\.|fetch\\s*\\(|' +
        'https?\\.(?:request|get)|socket\\.)',
    ).test(script)
  )
    return { reason: 'network' }
  if (
    new RegExp(
      '\\b(?:os\\.(?:remove|unlink|rmdir)|shutil\\.rmtree|' +
        'fs\\.(?:rm|unlink|rmdir)|Deno\\.remove)\\b',
    ).test(script)
  )
    return { reason: 'file_delete' }
  return null
}
/** Lexical containment; canonicalize symlinks before actual I/O. */
export function within(path: string, roots: readonly string[]): boolean {
  const normalize = (raw: string): string | null => {
    const value = raw.replaceAll('\\', '/')
    if (!value.startsWith('/') && !/^[a-z]:\//i.test(value)) return null
    const windows = /^[a-z]:\//i.test(value) || value.startsWith('//')
    const prefix = /^[a-z]:\//i.test(value)
      ? value.slice(0, 2)
      : value.startsWith('//')
        ? '//'
        : ''
    const stack: string[] = []
    for (const part of value.slice(prefix.length).split('/')) {
      if (!part || part === '.') continue
      if (part === '..') {
        if (!stack.length) return null
        stack.pop()
      } else stack.push(part)
    }
    const result = `${prefix}/${stack.join('/')}`.replace(/^\/\/\//, '//')
    return windows ? result.toLowerCase() : result
  }
  const target = normalize(path)
  if (target === null) return false
  return roots.some((root) => {
    const base = normalize(root)
    return (
      base !== null &&
      (target === base || target.startsWith(base.replace(/\/$/, '') + '/'))
    )
  })
}
