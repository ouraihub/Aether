import type { ProviderPreset } from '@ouraihub/aether-ui/ports'

/** Public, non-secret provider templates for China-focused hosts. */
export const CHINA_PROVIDER_PRESETS: readonly ProviderPreset[] = [
  {
    id: 'qiniu',
    name: '七牛云',
    baseUrl: 'https://api.qnaigc.com',
    protocol: 'openai-chat',
    apiKeyUrl: 'https://s.qiniu.com/BfyeU3',
  },
  {
    id: 'siliconflow',
    name: '硅基流动',
    baseUrl: 'https://api.siliconflow.cn/v1',
    protocol: 'openai-chat',
    apiKeyUrl: 'https://cloud.siliconflow.cn/i/ieP7r6Bx',
  },
  {
    id: 'fenno',
    name: 'Fenno',
    baseUrl: 'https://api.fenno.ai/v1',
    protocol: 'openai-chat',
    apiKeyUrl: 'https://api.fenno.ai/s/vWbM',
  },
]
