<script>
  // Native features retain their query flags. Explicit ?web=1 connects only to the
  // local Worker via Vite; its manually entered demo bearer stays in page memory.
  import { createDefaultNotify, createI18n, KitNotifier, setKitContext } from '@ouraihub/aether-ui'
  import { ChatPanel } from '@ouraihub/aether-ui/chat'
  import { createLogger, LogPanel, LogSettings } from '@ouraihub/aether-ui/logs'
  import { McpClientSettings } from '@ouraihub/aether-ui/mcp'
  import { ProvidersSettings } from '@ouraihub/aether-ui/providers'
  import { SkillsSettings } from '@ouraihub/aether-ui/skills'
  import { createTauriPorts } from '@ouraihub/aether-ui/tauri'
  import { createWebPorts } from '@ouraihub/aether-ui/web'
  // Stands in for the host's own shadcn components; the package does not export its UI primitives.
  import { Button } from '../../../../packages/ui/src/components/ui/button/index.js'
  import * as Dialog from '../../../../packages/ui/src/components/ui/dialog/index.js'
  import { Input } from '../../../../packages/ui/src/components/ui/input/index.js'
  import * as Field from '../../../../packages/ui/src/components/ui/field/index.js'
  import * as Alert from '../../../../packages/ui/src/components/ui/alert/index.js'

  const query = new URLSearchParams(location.search)
  const conversations = query.get('history') !== '0'
  const skills = query.get('skills') === '1'
  const mcpClient = query.get('mcp') === '1'
  const web = query.get('web') === '1'
  const localWeb = import.meta.env.DEV && ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname)
  let token = $state('')
  let connected = $state(false)
  let connecting = $state(false)
  let connectionOpen = $state(true)
  let connectionError = $state('')
  let streamError = $state('')
  const webFetch = (input, init) => {
    if (!localWeb || !token || new URL(input).origin !== location.origin)
      throw new Error('Local Worker connection required')
    const headers = new Headers(init.headers)
    headers.set('Authorization', `Bearer ${token}`)
    return fetch(input, { ...init, headers })
  }
  const ports = web
    ? createWebPorts({
        baseURL: location.origin,
        fetch: webFetch,
        // This local example uses bearer actors, never cookie sessions or a fabricated CSRF token.
        credentials: 'omit',
        csrf: '',
        providers: true,
        chat: true,
        conversations,
        skills,
        mcpClient,
        onStreamError: (runId, error) => {
          streamError = `Run ${runId}: connection interrupted (${error.code}). Outcome is not confirmed; check history before retrying.`
        },
      })
    : createTauriPorts({ providers: true, chat: true, conversations, skills, mcpClient })
  const i18n = createI18n(navigator.language)
  const notify = createDefaultNotify()
  // Web has no LogsPort: keep diagnostic event names only, never request data or credentials.
  const logger = web
    ? {
        debug: (event) => console.debug(event),
        info: (event) => console.info(event),
        warn: (event) => console.warn(event),
        error: (event) => console.error(event),
      }
    : createLogger(ports.logs)
  setKitContext({ ports, i18n, notify, logger })

  let settings = $state(false)
  let skillsOpen = $state(false)
  let mcpOpen = $state(false)
  let chatOpen = $state(true)
  let logsOpen = $state(false)

  async function connect(event) {
    event.preventDefault()
    if (connecting || !localWeb || !token || /[\s,]/u.test(token)) return
    connecting = true
    connectionError = ''
    try {
      const response = await webFetch(new URL('/health', location.origin).href, {
        method: 'GET',
        headers: { Accept: 'application/json' },
        credentials: 'omit',
        redirect: 'error',
      })
      if (!response.ok) {
        connectionError = `Worker connection rejected (HTTP ${response.status}). Check the token and local Worker origin.`
        return
      }
      connected = true
      connectionOpen = false
    } catch {
      connectionError = 'Unable to reach the local Worker. Start it on 127.0.0.1:8787 and try again.'
    } finally {
      connecting = false
    }
  }
</script>

<main class="mx-auto flex h-screen max-w-5xl flex-col gap-4 p-6">
  <header class="flex items-center gap-3">
    <h1 class="text-xl font-semibold">Aether · Assistant</h1>
    {#if !web || connected}
      <Button class="ml-auto" variant="outline" onclick={() => (settings = true)}>{i18n.t('providers.title')}</Button>
      {#if skills}
        <Button variant="outline" onclick={() => (skillsOpen = true)}>{i18n.t('skills.title')}</Button>
      {/if}
      {#if mcpClient}
        <Button variant="outline" onclick={() => (mcpOpen = true)}>{i18n.t('mcp.client.title')}</Button>
      {/if}
      <Button variant="outline" onclick={() => (chatOpen = !chatOpen)}>{i18n.t('chat.title')}</Button>
      {#if !web}
        <Button variant="outline" onclick={() => (logsOpen = !logsOpen)}>{i18n.t('logs.title')}</Button>
      {/if}
    {:else}
      <Button class="ml-auto" variant="outline" onclick={() => (connectionOpen = true)}>Connect Worker</Button>
    {/if}
  </header>
  {#if web && streamError}
    <Alert.Root variant="destructive">
      <Alert.Title>Worker stream interrupted</Alert.Title>
      <Alert.Description>{streamError}</Alert.Description>
    </Alert.Root>
  {/if}
  {#if (!web || connected) && chatOpen}
    <ChatPanel
      permissionModes={web
        ? ['default', 'plan', 'full_access']
        : ['default', 'plan', 'accept_edits', 'auto', 'full_access']}
      onClose={() => (chatOpen = false)}
      {...web ? { onOpenSettings: () => (settings = true) } : {}}
    />
  {/if}
  {#if logsOpen}
    <LogSettings />
    <LogPanel />
  {/if}
</main>
{#if web && !connected}
  <Dialog.Root bind:open={connectionOpen}>
    <Dialog.Content>
      <Dialog.Header>
        <Dialog.Title>Connect local Worker</Dialog.Title>
        <Dialog.Description>
          API: {location.origin}/api/v1. Vite forwards to 127.0.0.1:8787. Enter your configured demo bearer token. It
          stays in memory until this page closes or reloads. Cookie authentication is not used.
        </Dialog.Description>
      </Dialog.Header>
      <form onsubmit={connect}>
        <Field.FieldGroup>
          <Field.Field data-invalid={!!connectionError}>
            <Field.FieldLabel for="worker-token">Worker bearer token</Field.FieldLabel>
            <Input
              id="worker-token"
              type="password"
              bind:value={token}
              required
              autocomplete="off"
              spellcheck={false}
              pattern="[^\s,]+"
              disabled={connecting || !localWeb}
              aria-invalid={!!connectionError}
              aria-describedby="worker-connection-status"
            />
            <Field.FieldDescription id="worker-connection-status" role="status">
              {localWeb ? connectionError : 'Web demo connections are available only on a loopback Vite dev server.'}
            </Field.FieldDescription>
          </Field.Field>
          <Button type="submit" disabled={connecting || !localWeb || !token}>
            {connecting ? 'Connecting…' : 'Connect'}
          </Button>
        </Field.FieldGroup>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}
{#if !web || connected}
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
{/if}
