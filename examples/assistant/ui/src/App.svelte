<script>
  // Host root (docs/16 §3): providers, skills and MCP client settings in dialogs, a closable
  // ChatPanel, logs. `?history=0` matches a backend started with KIT_NO_HISTORY=1 (no
  // conversation store); `?skills=1` / `?mcp=1` builds with the `skills` / `mcp-client` feature.
  import { createDefaultNotify, createI18n, KitNotifier, setKitContext } from '@ouraihub/aether-ui'
  import { ChatPanel } from '@ouraihub/aether-ui/chat'
  import { createLogger, LogPanel, LogSettings } from '@ouraihub/aether-ui/logs'
  import { McpClientSettings } from '@ouraihub/aether-ui/mcp'
  import { ProvidersSettings } from '@ouraihub/aether-ui/providers'
  import { SkillsSettings } from '@ouraihub/aether-ui/skills'
  import { createTauriPorts } from '@ouraihub/aether-ui/tauri'
  // Stands in for the host's own shadcn components; the package does not export its UI primitives.
  import { Button } from '../../../../packages/ui/src/components/ui/button/index.js'
  import * as Dialog from '../../../../packages/ui/src/components/ui/dialog/index.js'

  const query = new URLSearchParams(location.search)
  const chatHistory = query.get('history') !== '0'
  const skills = query.get('skills') === '1'
  const mcpClient = query.get('mcp') === '1'
  const ports = createTauriPorts({ providers: true, chat: true, chatHistory, skills, mcpClient })
  const i18n = createI18n(navigator.language)
  const notify = createDefaultNotify()
  const logger = createLogger(ports.logs)
  setKitContext({ ports, i18n, notify, logger })

  let settings = $state(false)
  let skillsOpen = $state(false)
  let mcpOpen = $state(false)
  let chatOpen = $state(true)
  let logsOpen = $state(false)
</script>

<main class="mx-auto flex h-screen max-w-5xl flex-col gap-4 p-6">
  <header class="flex items-center gap-3">
    <h1 class="text-xl font-semibold">Aether · Assistant</h1>
    <Button class="ml-auto" variant="outline" onclick={() => (settings = true)}>{i18n.t('providers.title')}</Button>
    {#if skills}
      <Button variant="outline" onclick={() => (skillsOpen = true)}>{i18n.t('skills.title')}</Button>
    {/if}
    {#if mcpClient}
      <Button variant="outline" onclick={() => (mcpOpen = true)}>{i18n.t('mcp.client.title')}</Button>
    {/if}
    <Button variant="outline" onclick={() => (chatOpen = !chatOpen)}>{i18n.t('chat.title')}</Button>
    <Button variant="outline" onclick={() => (logsOpen = !logsOpen)}>{i18n.t('logs.title')}</Button>
  </header>
  {#if chatOpen}
    <ChatPanel onClose={() => (chatOpen = false)} />
  {/if}
  {#if logsOpen}
    <LogSettings />
    <LogPanel />
  {/if}
</main>
<Dialog.Root bind:open={settings}>
  <Dialog.Content class="sm:max-w-4xl">
    <Dialog.Header>
      <Dialog.Title>{i18n.t('providers.title')}</Dialog.Title>
    </Dialog.Header>
    <div class="max-h-[70vh] overflow-y-auto"><ProvidersSettings /></div>
  </Dialog.Content>
</Dialog.Root>
<Dialog.Root bind:open={skillsOpen}>
  <Dialog.Content class="sm:max-w-4xl">
    <Dialog.Header>
      <Dialog.Title>{i18n.t('skills.title')}</Dialog.Title>
    </Dialog.Header>
    <div class="max-h-[70vh] overflow-y-auto"><SkillsSettings /></div>
  </Dialog.Content>
</Dialog.Root>
<Dialog.Root bind:open={mcpOpen}>
  <Dialog.Content class="sm:max-w-4xl">
    <Dialog.Header>
      <Dialog.Title>{i18n.t('mcp.client.title')}</Dialog.Title>
    </Dialog.Header>
    <div class="max-h-[70vh] overflow-y-auto"><McpClientSettings /></div>
  </Dialog.Content>
</Dialog.Root>
<KitNotifier {notify} {i18n} />
