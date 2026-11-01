<script lang="ts">
  import { setKitContext } from './src/context'
  import { createI18n } from './src/i18n'
  import { createFakePorts } from './src/ports/fake'
  import ChatPanel from './src/chat/ChatPanel.svelte'
  import ProvidersSettings from './src/providers/ProvidersSettings.svelte'
  import SearchSettings from './src/search/SearchSettings.svelte'
  import McpClientSettings from './src/mcp/McpClientSettings.svelte'
  import { Button } from './src/components/ui/button/index.js'
  const ports = createFakePorts({
    providers: {
      modelOptions: async () => [{ endpointId: 'e1', modelId: 'm1', label: 'Smoke model', modelType: 'chat', name: null, supportsTools: true, reasoning: null, nativeWebSearch: false, canVision: false }],
    },
    search: {
      settings: async (capability) => ({ capability, defaultProvider: 'jina', preferences: { maxResults: 5, compression: 'none', cutoffLimit: 2000, blacklist: [], modelToolsPreferred: false }, providers: [{ id: 'jina', name: 'Jina', description: 'Reader', keyMode: 'optional', basicAuth: false, keys: [], ready: true, defaultBaseUrl: 'https://jina.ai' }] }),
    },
  })
  setKitContext({ ports, i18n: createI18n('en'), notify: { toast: (kind, text) => { notice = `${kind}: ${text}` }, confirm: async () => true }, logger: { debug() {}, info() {}, warn() {}, error() {} } })
  let page = $state('chat')
  let notice = $state('')
</script>
<main style="height:100vh;padding:20px;display:flex;flex-direction:column;gap:12px">
  <nav>{#each ['chat', 'providers', 'search', 'mcp'] as item}<Button variant="outline" onclick={() => page = item}>{item}</Button>{/each}</nav>
  <p role="status">{notice}</p>
  {#if page === 'chat'}<ChatPanel model={{ endpointId: 'e1', modelId: 'm1' }} />{:else if page === 'providers'}<ProvidersSettings />{:else if page === 'search'}<SearchSettings />{:else}<McpClientSettings />{/if}
</main>
