import type {
  ModelType,
  Protocol,
  ReasoningConfig,
} from '@ouraihub/aether-contracts'

export interface Inferred {
  modelType: ModelType
  contextWindow: number | null
  maxOutput: number | null
  canVision: boolean
  supportsTools: boolean | null
  group: string | null
}

type Limits = readonly [
  contextWindow: number,
  maxOutput: number,
  vision: boolean,
]

const TYPES: readonly (readonly [readonly string[], ModelType])[] = [
  [['embed'], 'embedding'],
  [['rerank'], 'rerank'],
  [['tts', 'speech'], 'tts'],
  [['whisper', 'stt', 'transcri'], 'stt'],
  [['dall-e', 'image', 'flux', 'imagen'], 'image'],
  [['video', 'sora', 'veo'], 'video'],
]

const FAMILIES: readonly (readonly [
  prefix: string,
  group: string,
  limits: Limits,
])[] = [
  ['claude', 'claude', [200_000, 8_192, true]],
  ['gpt-3.5', 'gpt', [16_385, 4_096, false]],
  ['gpt', 'gpt', [128_000, 16_384, true]],
  ['gemini', 'gemini', [1_048_576, 8_192, true]],
  ['deepseek', 'deepseek', [128_000, 8_192, false]],
  ['qwen', 'qwen', [131_072, 8_192, false]],
  ['glm', 'glm', [128_000, 4_096, false]],
  ['kimi', 'kimi', [131_072, 8_192, false]],
  ['moonshot-v1-8k', 'moonshot', [8_192, 4_096, false]],
  ['moonshot-v1-32k', 'moonshot', [32_768, 4_096, false]],
  ['moonshot', 'moonshot', [131_072, 4_096, false]],
]

const O_SERIES: Limits = [200_000, 100_000, true]

const VISION_MARKERS = ['vision', '-vl', 'vl-', '4v'] as const

export function inferModel(modelId: string): Inferred {
  const id = modelId.toLowerCase()
  const modelType =
    TYPES.find(([words]) => words.some((word) => id.includes(word)))?.[1] ??
    'chat'
  const name = id.slice(id.lastIndexOf('/') + 1)
  const known = FAMILIES.find(([prefix]) => name.startsWith(prefix))
  const family: readonly [string, Limits] | undefined = known
    ? [known[1], known[2]]
    : /^o\d/.test(name)
      ? ['o', O_SERIES]
      : undefined
  const group = family?.[0] ?? null
  const marked = VISION_MARKERS.some((marker) => name.includes(marker))
  if (family && modelType === 'chat') {
    const [contextWindow, maxOutput, canVision] = family[1]
    return {
      modelType,
      contextWindow,
      maxOutput,
      canVision: canVision || marked,
      supportsTools: true,
      group,
    }
  }
  return {
    modelType,
    contextWindow: null,
    maxOutput: null,
    canVision: marked && modelType === 'chat',
    supportsTools: null,
    group,
  }
}

// ponytail: bounded known-family rules; add a catalog only when provider-specific metadata is needed.
export function inferReasoning(
  modelId: string,
  protocol: Protocol,
  baseUrl: string,
  maxOutput: number | null,
): ReasoningConfig | null {
  const name = modelId.toLowerCase().split('/').at(-1) ?? ''
  const normalized = name.replaceAll('.', '-')
  if (protocol === 'openai-chat') {
    const vendor = vendorReasoning(normalized, baseUrl)
    if (vendor) return vendor
  }
  const matches = (prefix: string): boolean => {
    if (normalized === prefix) return true
    if (!normalized.startsWith(`${prefix}-`)) return false
    const suffix = normalized.slice(prefix.length + 1).split('-')[0] ?? ''
    return !/^\d+$/.test(suffix) || suffix.length >= 4
  }
  const claude = (budgetOutput: number | null): ReasoningConfig | null => {
    const families = ['opus', 'sonnet', 'haiku']
    const newest = families.some((family) => matches(`claude-${family}-5-5`))
    const adaptive =
      newest ||
      families.some((family) => matches(`claude-${family}-5`)) ||
      ['opus', 'sonnet'].some((family) =>
        ['6', '7', '8'].some((minor) => matches(`claude-${family}-4-${minor}`)),
      )
    if (adaptive)
      return {
        dialect: 'anthropic-adaptive',
        efforts: [
          ...(matches('claude-opus-5-5') ? [] : (['none'] as const)),
          'low',
          'medium',
          'high',
          ...(newest ? (['xhigh'] as const) : []),
          'max',
        ],
      }
    const budget = [
      'claude-3-7-sonnet',
      'claude-sonnet-4-5',
      'claude-sonnet-4',
      'claude-opus-4-5',
      'claude-opus-4-1',
      'claude-opus-4',
      'claude-haiku-4-5',
    ].some(matches)
    const budgetTokens = Math.min(8192, (budgetOutput ?? 8193) - 1)
    if (budget && budgetTokens >= 1024)
      return {
        dialect: 'anthropic-budget',
        efforts: ['none', 'low', 'medium', 'high'],
        budgetTokens,
      }
    return null
  }
  if (protocol === 'anthropic-messages') return claude(maxOutput)
  // OpenAI Chat/Responses lines (official or relay) send `reasoning_effort`; a known family keeps its efforts there.
  const openai = (efforts: ReasoningConfig['efforts']): ReasoningConfig => ({
    dialect: 'openai',
    efforts,
  })
  if (normalized.startsWith('claude')) {
    const config = claude(null)
    return config && openai(config.efforts)
  }
  if (normalized.includes('non-reasoning')) return null
  const any = (...prefixes: string[]): boolean => prefixes.some(matches)
  if (
    (matches('o1') &&
      !normalized.includes('-mini') &&
      !normalized.includes('-preview')) ||
    any('o3', 'o4-mini', 'gpt-oss')
  )
    return openai(['low', 'medium', 'high'])
  if (any('grok-4-3')) return openai(['none', 'low', 'medium', 'high'])
  if (any('grok-4-6', 'grok-4-7'))
    return openai(['low', 'medium', 'high', 'xhigh'])
  if (any('grok-3-mini')) return openai(['low', 'high'])
  if (any('gpt-5-1-codex-max')) return openai(['medium', 'high', 'xhigh'])
  if (any('gpt-5-1-codex')) return openai(['medium', 'high'])
  if (any('gpt-5-codex')) return openai(['low', 'medium', 'high'])
  if (
    any(
      'gpt-5-2-codex',
      'gpt-5-3-codex',
      'gpt-5-4-codex',
      'gpt-5-5-codex',
      'gpt-5-6-codex',
    )
  )
    return openai(['low', 'medium', 'high', 'xhigh'])
  if (/-(?:chat|pro|codex)/.test(normalized)) return null
  if (any('gpt-5-1')) return openai(['none', 'low', 'medium', 'high'])
  if (any('gpt-5-2', 'gpt-5-3', 'gpt-5-4', 'gpt-5-5', 'gpt-5-6'))
    return openai(['none', 'low', 'medium', 'high', 'xhigh'])
  if (any('gpt-5')) return openai(['minimal', 'low', 'medium', 'high'])
  return null
}

/** Official vendor endpoints switch reasoning with their own fields (docs/28 RT4); a relay serving the same model keeps the OpenAI dialect. */
function vendorReasoning(
  name: string,
  baseUrl: string,
): ReasoningConfig | null {
  const host =
    (baseUrl.split('://').at(-1) ?? '').split(/[/:?]/)[0]?.toLowerCase() ?? ''
  const on = (...domains: string[]): boolean =>
    domains.some((domain) => host === domain || host.endsWith(`.${domain}`))
  const starts = (...prefixes: string[]): boolean =>
    prefixes.some((prefix) => name.startsWith(prefix))
  const thinking = (efforts: ReasoningConfig['efforts']): ReasoningConfig => ({
    dialect: 'thinking-type',
    efforts,
  })
  const toggle = thinking(['none', 'auto'])
  if (on('deepseek.com')) {
    if (starts('deepseek-v4', 'deepseek-v5'))
      return thinking(['none', 'low', 'high', 'max'])
    return starts('deepseek-chat', 'deepseek-v3') ? toggle : null
  }
  if (on('bigmodel.cn', 'z.ai')) {
    if (starts('glm-5-3')) return thinking(['low', 'high', 'max'])
    return starts('glm-5', 'glm-4-5', 'glm-4-6', 'glm-4-7') ? toggle : null
  }
  if (on('volces.com')) return starts('doubao-seed') ? toggle : null
  if (on('moonshot.cn', 'moonshot.ai')) {
    if (starts('kimi-k3')) return thinking(['none', 'low', 'high', 'max'])
    return starts(
      'kimi-k2-5',
      'kimi-k2-6',
      'kimi-k2-7',
      'kimi-k2-8',
      'kimi-k2-9',
    ) && !name.includes('-code')
      ? toggle
      : null
  }
  if (
    on('aliyuncs.com') &&
    name.startsWith('qwen3') &&
    !name.startsWith('qwen3-8-') &&
    !/instruct|coder|thinking/.test(name)
  )
    return {
      dialect: 'qwen',
      efforts: ['none', 'auto', 'low', 'medium', 'high', 'max'],
      budgetTokens: starts('qwen3-max', 'qwen3-5', 'qwen3-6', 'qwen3-7')
        ? 81_920
        : 38_912,
    }
  return null
}
